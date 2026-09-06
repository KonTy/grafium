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
    /// When [`Self::summary`] is `None` because summary generation was
    /// *attempted and failed* (as opposed to skipped because the page
    /// was too short or empty), the human-readable failure reason from
    /// the LLM/parser lives here so the UI can surface it in an actionable
    /// error card instead of silently rendering nothing.
    /// `None` when the summary either succeeded or was never attempted
    /// (e.g. `eligible_blocks.is_empty()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_error: Option<String>,
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
        cancel: &crate::cancel::CancellationToken,
    ) -> Result<PageReferencesMeta> {
        let mut all_refs: Vec<GeneratedReference> = Vec::new();
        let mut ref_counter = 1;
        let eligible_blocks = blocks
            .iter()
            .filter(|(_, content)| content.trim().len() >= 20)
            .map(|(block_id, content)| (block_id.as_str(), content.as_str()))
            .collect::<Vec<_>>();

        // Tell the user which compute backend the LLM is on *before*
        // we start burning cycles — a CPU-fallback warning here is
        // exactly the context they need to decide whether to wait it
        // out or cancel and reconfigure the model. `LlmProvider` has a
        // default `None` for non-local providers, so this line simply
        // doesn't show for cloud LLMs (Ollama/OpenAI/etc), where the
        // question of "GPU vs CPU" is out of the app's hands anyway.
        if let Some(summary) = llm.backend_summary() {
            on_progress(&summary);
        }
        on_progress(&format!(
            "Analyzing {} block{} for key concepts (this can take a while on a local CPU \
             model)...",
            eligible_blocks.len(),
            if eligible_blocks.len() == 1 { "" } else { "s" }
        ));

        let mut pending_references = Vec::new();
        // Concept extraction is best-effort — the same "still ship a
        // summary" reasoning as the summarization block below applies:
        // an LLM/context-creation failure during concept extraction (e.g.
        // an out-of-VRAM `null reference from llama.cpp` on a long
        // imported transcript) shouldn't discard the summary the user
        // could still get from this same "Analyze this Page" click.
        // Chunking (see `extract_concepts_batch`) already makes this
        // failure rare; when it does still happen, degrade to
        // "no cross-references, but here's the summary" instead of
        // failing the whole operation.
        match self
            .extract_concepts_batch(&eligible_blocks, llm, on_progress, cancel)
            .await
        {
            Ok(per_block_concepts) => {
                for ((block_id, _), concepts) in
                    eligible_blocks.iter().copied().zip(per_block_concepts.into_iter())
                {
                    for concept in concepts {
                        pending_references.push(PendingReference {
                            block_id: block_id.to_string(),
                            concept,
                        });
                    }
                }
            }
            Err(error) => {
                on_progress(&format!(
                    "Skipping cross-references (concept extraction failed): {error}"
                ));
                tracing::warn!(
                    target: "grafium_core::ai::references",
                    "concept extraction failed for page {page_id}: {error}"
                );
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
            // Same best-effort reasoning as the concept extraction above:
            // an embedder-side context-creation OOM here shouldn't kill
            // the summary the user could still get. The vector-search
            // step is only useful *because* it turns concept embeddings
            // into related pages, so if we couldn't produce embeddings
            // we simply skip that whole step and continue to summary.
            match embedder.embed_queries(&query_texts).await {
                Ok(vecs) => vecs,
                Err(error) => {
                    on_progress(&format!(
                        "Skipping cross-references (embedding failed): {error}"
                    ));
                    tracing::warn!(
                        target: "grafium_core::ai::references",
                        "concept embedding failed for page {page_id}: {error}"
                    );
                    pending_references.clear();
                    Vec::new()
                }
            }
        };
        if embeddings.len() != query_texts.len() && !pending_references.is_empty() {
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
        // the references work already done above. When it does fail we
        // stash the reason in `summary_error` so the UI can surface an
        // actionable "here's why there's no summary" card instead of
        // silently rendering nothing.
        let (summary, summary_error) = if eligible_blocks.is_empty() {
            (None, None)
        } else {
            on_progress("Summarizing article...");
            let full_text = eligible_blocks
                .iter()
                .map(|(_, content)| *content)
                .collect::<Vec<_>>()
                .join("\n\n");
            match generate_page_summary(page_title, &full_text, llm, on_progress, cancel).await {
                Ok(summary) => (Some(summary), None),
                Err(error) => {
                    let message = error.to_string();
                    on_progress(&format!("Could not generate a summary: {message}"));
                    (None, Some(message))
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
            summary_error,
        })
    }

    /// Use LLM to extract key concepts/entities from a text block.
    async fn extract_concepts(
        &self,
        content: &str,
        llm: &dyn LlmProvider,
        on_progress: &mut (dyn FnMut(&str) + Send),
        cancel: &crate::cancel::CancellationToken,
    ) -> Result<Vec<ConceptExtraction>> {
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: CONCEPT_EXTRACTION_PROMPT.to_string(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: append_no_think_directive(content),
            },
        ];

        let response = stream_completion(
            llm,
            &messages,
            &concept_extraction_options(),
            "Extracting concepts: ",
            on_progress,
            cancel,
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
        cancel: &crate::cancel::CancellationToken,
    ) -> Result<Vec<Vec<ConceptExtraction>>> {
        if blocks.is_empty() {
            return Ok(Vec::new());
        }

        if blocks.len() == 1 {
            return Ok(vec![
                self.extract_concepts(blocks[0].1, llm, on_progress, &crate::cancel::CancellationToken::new())
                    .await?,
                self.extract_concepts(blocks[0].1, llm, on_progress, cancel)
                    .await?,
            ]);
        }

        // Split the input by cumulative content size before making any
        // LLM calls: an imported YouTube/podcast transcript can be 100+
        // 30-second-chunk blocks, which serialized into one JSON prompt
        // reaches ~6-10k tokens. A single prompt that large forces
        // `local_llm::generate` to size `n_batch` — the compute-buffer
        // reservation llama.cpp does at context-creation time — off
        // that same length, which on a Vulkan backend already holding a
        // large chat model's weights + KV cache is enough to push VRAM
        // past capacity: `llama_new_context_with_model` returns NULL
        // and the whole "Analyze this Page" fails with the raw
        // "failed to create llama context: null reference from
        // llama.cpp" the upstream binding surfaces. Chunking here keeps
        // each individual concept-extraction call's prompt small enough
        // that its context allocation succeeds even under VRAM pressure,
        // at the cost of a few extra sequential calls for long pages.
        let chunks = chunk_blocks_by_content_size(blocks, MAX_CONCEPT_BATCH_INPUT_CHARS);
        let mut extracted: Vec<Vec<ConceptExtraction>> = Vec::with_capacity(blocks.len());
        for (chunk_index, chunk) in chunks.iter().enumerate() {
            if chunks.len() > 1 {
                on_progress(&format!(
                    "Extracting concepts (batch {}/{}): {} block{}...",
                    chunk_index + 1,
                    chunks.len(),
                    chunk.len(),
                    if chunk.len() == 1 { "" } else { "s" }
                ));
            }
            extracted.extend(
                self.extract_concepts_batch_single(chunk, llm, on_progress, cancel)
                    .await?,
            );
        }
        Ok(extracted)
    }

    /// One LLM call's worth of batched concept extraction: sends `blocks`
    /// as a single JSON prompt, parses the response, and falls back to
    /// per-block extraction if the batched JSON is unparseable. The
    /// per-chunk sizing that keeps the prompt within a safe context
    /// budget is `extract_concepts_batch`'s job, not this one.
    async fn extract_concepts_batch_single(
        &self,
        blocks: &[(&str, &str)],
        llm: &dyn LlmProvider,
        on_progress: &mut (dyn FnMut(&str) + Send),
        cancel: &crate::cancel::CancellationToken,
    ) -> Result<Vec<Vec<ConceptExtraction>>> {
        if blocks.is_empty() {
            return Ok(Vec::new());
        }
        if blocks.len() == 1 {
            return Ok(vec![
                self.extract_concepts(blocks[0].1, llm, on_progress, cancel)
                    .await?,
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
                content: append_no_think_directive(&request_body),
            },
        ];

        let response = stream_completion(
            llm,
            &messages,
            &concept_extraction_options(),
            "Extracting concepts: ",
            on_progress,
            cancel,
        )
        .await?;
        match parse_batched_concept_response(&response, blocks) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                let mut extracted = Vec::with_capacity(blocks.len());
                for (_, content) in blocks {
                    extracted
                        .push(self.extract_concepts(content, llm, on_progress, cancel).await?);
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
    cancel: &crate::cancel::CancellationToken,
) -> Result<String> {
    // Flush a progress update at least this often (in wall-clock time),
    // so slow-generating models still show a heartbeat instead of a
    // silent gap that looks indistinguishable from a hang.
    const MIN_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
    // Also flush whenever this many *new* characters have been produced
    // since the last flush, so the UI feels responsive on fast models
    // that would otherwise batch too coarsely.
    const FLUSH_CHUNK_CHARS: usize = 40;
    // How much of the growing answer to keep in the progress message.
    // Long enough that the user can watch the summary being written;
    // short enough that the UI doesn't have to re-layout a runaway
    // string every time a token arrives. Older text is elided from the
    // left, not dropped from the full result.
    const PROGRESS_TAIL_CHARS: usize = 240;
    // How long we wait before starting to insist "this is taking a
    // while — press Cancel if you'd rather stop". Deliberately generous
    // (10 min): a slow-but-progressing summary on a partially-offloaded
    // 27B model can legitimately need this much time. Below this we
    // stay quiet (or emit the healthy per-token progress). Above this
    // we escalate the wording of the periodic heartbeat so the user
    // knows the app is *waiting on them*, not silently hung. Combined
    // with the Cancel button in the UI this replaces the old hard
    // 600s auto-abort — the user asked for "keep going, but let me
    // decide when to stop" and this is that.
    const SLOW_WARNING_SECS: u64 = 600;
    // If no token has arrived by this point, something is almost
    // certainly wrong — a healthy local model on a modest CPU gets to
    // its first token in tens of seconds, not minutes. We don't abort
    // here — the user is in control via the Cancel button — we just
    // surface a warning so they can decide whether to keep waiting or
    // stop.
    const NO_TOKEN_WARNING_SECS: u64 = 120;

    let start = std::time::Instant::now();
    let mut buffer = String::new();
    let mut token_count: usize = 0;
    let mut last_flush = std::time::Instant::now();
    let mut chars_since_flush: usize = 0;
    let mut warned_slow = false;
    // Shared counters the outer `select!` branches (timeout error,
    // heartbeat) read while the closure below owns `token_count` /
    // `total_chars` by move. `Ordering::Relaxed` is fine: this is
    // display-only bookkeeping, not synchronization; the closure runs
    // on the same executor as the select loop, so any consistency
    // requirement beyond "eventually visible" is overkill.
    let token_count_shared =
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let total_chars_shared =
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let token_count_closure = token_count_shared.clone();
    let total_chars_closure = total_chars_shared.clone();

    // Emit an initial "sent the prompt, now waiting" message so the UI
    // shows something the *instant* stream_completion is entered rather
    // than only after the first token arrives — a slow-thinking model
    // (e.g. a partially-GPU-offloaded 27B running on 8 GB of free VRAM)
    // can easily take 20-60 seconds to produce the first token, and
    // that silent gap is precisely what makes the app look hung.
    // Deliberately phrased around user control ("press Cancel to stop"),
    // not a fixed timeout — the UI's Cancel button is the escape hatch
    // now, not a hardcoded ceiling.
    on_progress(&format!(
        "{label}waiting for model to respond… (press Cancel any time to stop)"
    ));

    // If the provider already knows which backend it landed on (i.e.
    // the model has been loaded on a previous request in this session),
    // surface it up front so the user sees *why* generation is fast or
    // slow before the tokens start arriving. On the very first request
    // the load hasn't happened yet, so `backend_summary()` returns None
    // and we stay quiet — the load-info emit at the *end* of the
    // completion (below) picks it up on that first run instead. This
    // is the "am I on GPU or CPU?" question the user has been asking:
    // now it shows up directly in the progress panel.
    let backend_known_upfront = llm.backend_summary().is_some();
    if let Some(bs) = llm.backend_summary() {
        on_progress(&format!("{label}{bs}"));
    }

    // Snapshot the tap cutoff *now* so any llama.cpp/GGML log lines the
    // model emits during this specific completion can be surfaced in
    // heartbeat / error messages verbatim (rather than dumping the whole
    // session's log). Kept even though the hard-timeout branch is gone,
    // because the cancel branch may still want to include it.
    #[allow(unused_variables)]
    let completion_started_at = std::time::Instant::now();

    // Progress messages are fanned into a single channel so both the
    // per-token callback (`on_token`) and the periodic heartbeat can
    // enqueue updates without either of them holding a `&mut` borrow of
    // `on_progress`. A dedicated `recv` branch in the `select!` loop
    // below is the *only* thing that actually calls `on_progress`, which
    // sidesteps the borrow-checker conflict cleanly and — as a bonus —
    // makes the heartbeat truly independent of whether tokens are
    // arriving (the original bug: heartbeats going through `on_token`
    // never fired when no tokens ever showed up).
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let progress_tx_token = progress_tx.clone();

    let mut on_token = move |piece: &str| {
        buffer.push_str(piece);
        let total_chars = buffer.chars().count();
        token_count += 1;
        chars_since_flush += piece.chars().count();
        total_chars_closure
            .store(total_chars, std::sync::atomic::Ordering::Relaxed);
        token_count_closure
            .store(token_count, std::sync::atomic::Ordering::Relaxed);
        let now = std::time::Instant::now();
        if chars_since_flush >= FLUSH_CHUNK_CHARS
            || now.duration_since(last_flush) >= MIN_FLUSH_INTERVAL
        {
            let elapsed = start.elapsed().as_secs_f64().max(0.001);
            let rate = token_count as f64 / elapsed;
            // Show the tail of the growing answer so the user watches
            // the summary being written in real time; prefix with a
            // token count + rate + total elapsed time so they know it's
            // live even when the model briefly stalls between sentences.
            let tail = if total_chars > PROGRESS_TAIL_CHARS {
                let skip = total_chars - PROGRESS_TAIL_CHARS;
                let mut s = String::from("…");
                s.push_str(
                    &buffer
                        .chars()
                        .skip(skip)
                        .collect::<String>(),
                );
                s
            } else {
                buffer.clone()
            };
            let elapsed_secs = elapsed as u64;
            // One-shot slow-start warning: if the model finally sent a
            // token but the first-token latency was way outside the
            // "healthy" band, tell the user so they know why they were
            // staring at a silent dialog for so long. Doesn't repeat.
            if !warned_slow && token_count == 1 && elapsed_secs >= NO_TOKEN_WARNING_SECS {
                let _ = progress_tx_token.send(format!(
                    "{label}⚠ first token took {}s — this model/context is running much \
                     slower than a healthy setup; consider lowering context_size or \
                     GPU-offload layers in Settings",
                    elapsed_secs
                ));
                warned_slow = true;
            }
            let _ = progress_tx_token.send(format!(
                "{label}{token_count} tokens · {rate:.1} tok/s · {elapsed_secs}s elapsed\n{tail}"
            ));
            last_flush = now;
            chars_since_flush = 0;
        }
    };
    // Race the actual completion against a periodic "still working"
    // tick so the UI keeps updating even when no tokens have arrived
    // and against the caller-provided cancellation token so the user
    // can bail out at any point. There's *no* hard timeout branch
    // here anymore — the app used to auto-abort at 600s, but that
    // punished legitimately-slow-but-progressing models (a 27B on a
    // 16 GB card doing partial GPU offload can genuinely need 20+
    // minutes), so the UX became "you waited 10 minutes and got a
    // scary error message instead of a summary". Now the escalating
    // heartbeat + the always-visible Cancel button in the UI put the
    // decision back in the user's hands.
    let response = {
        // Give up immediately if the caller already cancelled between
        // constructing the messages and getting here — no reason to
        // spin up the worker just to kill it two microseconds later.
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let mut completion = Box::pin(
            llm.complete_stream(messages, options, &mut on_token),
        );
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(5));
        heartbeat.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                r = &mut completion => {
                    // Drain any progress messages `on_token` produced
                    // in its final flush before the future resolved,
                    // so the last tokens actually reach the UI (not
                    // just get lost in the channel when we exit).
                    while let Ok(msg) = progress_rx.try_recv() {
                        on_progress(&msg);
                    }
                    // First-load path: the worker just spawned + loaded
                    // the model during this very call. Now that the
                    // backend is known, tell the user which one (and
                    // how long it took) so subsequent generations feel
                    // predictable rather than "mystery box". Skip if
                    // we already emitted it upfront (subsequent
                    // requests) or if the provider still can't say.
                    if !backend_known_upfront {
                        if let Some(bs) = llm.backend_summary() {
                            on_progress(&format!("{label}{bs}"));
                        }
                    }
                    break r?;
                }
                _ = cancel.cancelled() => {
                    // User pressed Cancel on the progress toast. Flush
                    // whatever progress is queued so they see the final
                    // pre-cancel state, then hard-kill the worker —
                    // llama.cpp inference runs unmanaged C++ that
                    // checks nothing between tokens, so co-operatively
                    // "asking" it to stop wouldn't take effect until
                    // it finished on its own (i.e. never, for the
                    // hung-model case that motivated the cancel
                    // button). The kill causes `complete_stream`'s
                    // reader to return an error which we deliberately
                    // discard here — the caller wants "you cancelled
                    // this", not "the worker crashed mid-generation".
                    while let Ok(msg) = progress_rx.try_recv() {
                        on_progress(&msg);
                    }
                    on_progress(&format!("{label}cancelled by user"));
                    llm.abort_in_flight();
                    // Deliberately don't `.await` the completion here:
                    // once we kill the child, `complete_stream`'s inner
                    // reader will error and its future will resolve on
                    // its own timeline. Dropping our pinned box does
                    // the right thing (cancels the outer future).
                    return Err(CoreError::Cancelled);
                }
                Some(msg) = progress_rx.recv() => {
                    // Fan-out point: `on_token` and the heartbeat both
                    // enqueue here; this is the single place we borrow
                    // `on_progress` mutably. Without this branch the
                    // `select!` would starve either the heartbeat or
                    // the per-token stream.
                    on_progress(&msg);
                }
                _ = heartbeat.tick() => {
                    let toks = token_count_shared.load(std::sync::atomic::Ordering::Relaxed);
                    let elapsed = start.elapsed().as_secs();
                    if toks == 0 {
                        // Still waiting for the first token. Escalate
                        // wording so a genuinely-stuck run is obvious.
                        let note = if elapsed >= SLOW_WARNING_SECS {
                            " — this is unusually long, the model may be stuck; press Cancel to stop, or keep waiting"
                        } else if elapsed >= NO_TOKEN_WARNING_SECS {
                            " — well past a healthy first-token time; press Cancel to stop, or keep waiting"
                        } else {
                            ""
                        };
                        let _ = progress_tx.send(format!(
                            "{label}waiting for model to respond… {}s elapsed{note}",
                            elapsed
                        ));
                    } else if elapsed >= SLOW_WARNING_SECS {
                        // Tokens *are* flowing but the whole run has
                        // dragged on long enough that the user might
                        // want to reconsider. Nudge them via the
                        // heartbeat without interrupting the per-token
                        // progress that `on_token` is already emitting
                        // — a low-frequency reminder they can act on.
                        let chars = total_chars_shared.load(std::sync::atomic::Ordering::Relaxed);
                        let _ = progress_tx.send(format!(
                            "{label}still working… {elapsed}s elapsed, {toks} tokens, \
                             {chars} chars — press Cancel to stop, or keep waiting"
                        ));
                    }
                }
            }
        }
    };
    // Log the response envelope (length + short prefix, not the whole
    // body) so failed JSON extraction downstream can be diagnosed from
    // the log without having to reproduce the run — e.g. a
    // 7-character `<think>` response is diagnostic of the model
    // emitting EOS immediately after opening a reasoning tag, which
    // looks the same in the user-facing error as any other malformed
    // response. Kept at info level so it shows up in the default
    // production log (RUST_LOG=info) without env-var tweaking — this
    // is the only signal you get about the model's raw output when a
    // downstream parser silently rejects it.
    tracing::info!(
        target: "grafium_core::ai::references",
        "{label}response ready: len={} prefix={:?}",
        response.len(),
        super::truncate_to_char_boundary(response.trim(), 200),
    );
    Ok(response)
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
    cancel: &crate::cancel::CancellationToken,
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
            content: append_no_think_directive(&format!(
                "Title: {title}\n\nContent:\n{truncated_text}"
            )),
        },
    ];

    let response = stream_completion(
        llm,
        &messages,
        &summary_options(),
        "Summarizing: ",
        on_progress,
        cancel,
    )
    .await?;

    match parse_summary_response(&response) {
        Ok(summary) => Ok(summary),
        Err(structured_error) => {
            // The structured (JSON-envelope) attempt failed — usually
            // because the model's chat-template plumbing auto-injected
            // an unclosed `<think>` tag it never resolves, or because
            // an aggressively-quantized creative-writing fine-tune
            // just can't stay within a strict JSON schema. Rather than
            // give up, retry with a much simpler prompt that asks for
            // plain prose — any halfway-functional model can produce
            // *something* usable this way, and a plain-text summary is
            // strictly better than no summary at all. The one-shot
            // retry never repeats, so a genuinely broken model still
            // surfaces a fast, actionable error.
            on_progress(
                "Structured summary failed; retrying with plain-text prompt…",
            );
            tracing::debug!(
                target: "grafium_core::ai::references",
                "structured summary parse failed, retrying plain-text: {structured_error}"
            );

            let plain_messages = vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: PLAIN_SUMMARY_PROMPT.to_string(),
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: append_no_think_directive(&format!(
                        "Title: {title}\n\nContent:\n{truncated_text}"
                    )),
                },
            ];
            let plain_response = stream_completion(
                llm,
                &plain_messages,
                &summary_options(),
                "Summarizing (plain): ",
                on_progress,
                cancel,
            )
            .await?;

            let cleaned = strip_reasoning_block(plain_response.trim()).trim();
            let cleaned = cleaned
                .strip_prefix("<think>")
                .or_else(|| cleaned.strip_prefix("<thinking>"))
                .map(str::trim_start)
                .unwrap_or(cleaned);
            if is_substantive_summary_text(cleaned) {
                return Ok(PageSummary {
                    title_answer: None,
                    topics: vec![TopicSummary {
                        topic: "Summary".to_string(),
                        summary: cleaned.to_string(),
                        tags: Vec::new(),
                    }],
                });
            }

            // Both structured and plain prompts came back empty (usually
            // just a bare `<think>` and EOS). Some reasoning-mode models
            // get stuck in a fully deterministic "emit `<think>` → stop"
            // loop with a low temperature and the `/no_think` directive —
            // the exact combination the two attempts above both use.
            // Last-resort: drop the system prompt, drop the `/no_think`
            // directive (which some forks actually parse as *enabling*
            // reasoning-mode ceremony), crank the temperature to jolt the
            // sampler out of that deterministic loop, and cap max_tokens
            // low so we don't burn ten minutes on this recovery attempt.
            // If this ALSO comes back empty, the model is genuinely
            // broken for this content and we surface an error naming
            // exactly which model to swap in Settings.
            on_progress(
                "Plain-text summary also failed; last-resort retry with higher temperature…",
            );
            tracing::debug!(
                target: "grafium_core::ai::references",
                "plain summary also empty (response={:?}), attempting last-resort retry",
                super::truncate_to_char_boundary(plain_response.trim(), 200),
            );
            let last_resort_options = CompletionOptions {
                // Deliberately much lower than `summary_options`: this
                // is a "just give us anything usable" attempt, not a
                // full multi-topic summary — no reason to let it run
                // for thousands of tokens.
                max_tokens: Some(600),
                // High enough to break out of a deterministic `<think>`
                // → EOS loop the greedy/low-temp attempts got stuck in,
                // but not so high that the summary becomes creative
                // fiction unrelated to the content.
                temperature: Some(0.8),
                ..Default::default()
            };
            let last_resort_messages = vec![ChatMessage {
                role: MessageRole::User,
                content: format!(
                    "Write a short summary in 3-5 plain sentences of the following. \
                     Do not use any tags, JSON, markdown, or preamble — just the summary.\n\n\
                     Title: {title}\n\nContent:\n{truncated_text}"
                ),
            }];
            let last_resort_response = stream_completion(
                llm,
                &last_resort_messages,
                &last_resort_options,
                "Summarizing (last resort): ",
                on_progress,
                cancel,
            )
            .await?;
            let last_cleaned = strip_reasoning_block(last_resort_response.trim()).trim();
            let last_cleaned = last_cleaned
                .strip_prefix("<think>")
                .or_else(|| last_cleaned.strip_prefix("<thinking>"))
                .map(str::trim_start)
                .unwrap_or(last_cleaned);
            if is_substantive_summary_text(last_cleaned) {
                return Ok(PageSummary {
                    title_answer: None,
                    topics: vec![TopicSummary {
                        topic: "Summary".to_string(),
                        summary: last_cleaned.to_string(),
                        tags: Vec::new(),
                    }],
                });
            }

            // Three attempts all came back with nothing usable. Rewrap
            // the original error with the actionable "which model is
            // this" bit so the user knows exactly what to change in
            // Settings, not just "some model".
            let model_name = llm.name();
            let backend = llm
                .backend_summary()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            Err(CoreError::Parse(format!(
                "The currently loaded model \"{model_name}\"{backend} refused to \
                 produce a usable summary after three attempts (structured, plain-text, \
                 and last-resort). Its responses were either an unclosed `<think>` \
                 reasoning tag or a fragmentary preamble (\"Here\", \"Sure, here's a \
                 summary:\") and then EOS — usually a sign the model is a reasoning-mode \
                 or creative-writing fine-tune whose training got damaged during \
                 aggressive (IQ2_M / IQ3_XXS / etc.) quantization. Open Settings → Local \
                 LLM and pick a different model — a plain instruction-tuned chat model \
                 like Qwen3-4B-Instruct, Llama-3.1-8B-Instruct, or Mistral-7B-Instruct \
                 is a safe bet; avoid IQ2/IQ3 quantizations of \"Fable\", \"Fusion\", or \
                 other creative-writing fine-tunes. Original parse error: {structured_error}"
            )))
        }
    }
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

/// Rough upper bound on the *content* size of a single batched
/// concept-extraction LLM call. See the comment in
/// `ReferenceEngine::extract_concepts_batch` for why this exists at all
/// (imported YouTube/podcast transcripts routinely have enough
/// short blocks to overflow a small-model context window if serialised
/// into one prompt). Deliberately conservative: at ~4 chars/token this
/// caps the raw block content at ~1500 tokens, leaving comfortable room
/// for the JSON scaffolding, the system prompt, and the `max_tokens`
/// generation reserve even inside a modestly-sized (e.g. 4096-token)
/// context window. Enforced as an inclusive cumulative-length threshold,
/// not a hard cap: a single block whose own content already exceeds the
/// threshold is still sent on its own (there's nowhere smaller to split
/// it to), so this only ever spreads a many-block prompt across more
/// LLM calls, never truncates a block.
const MAX_CONCEPT_BATCH_INPUT_CHARS: usize = 6000;

/// Splits `blocks` into consecutive chunks so that each chunk's total
/// block-*content* character count stays at or below `max_chars`,
/// falling back to a single-block chunk whenever a block's own content
/// already exceeds the budget (rather than dropping/truncating it —
/// splitting a block would break the "concept text must appear verbatim
/// in that block's content" contract downstream).
fn chunk_blocks_by_content_size<'a, 'b>(
    blocks: &'a [(&'b str, &'b str)],
    max_chars: usize,
) -> Vec<Vec<(&'b str, &'b str)>> {
    let mut chunks: Vec<Vec<(&str, &str)>> = Vec::new();
    let mut current: Vec<(&str, &str)> = Vec::new();
    let mut current_chars: usize = 0;

    for &(block_id, content) in blocks {
        let block_chars = content.chars().count();
        // Start a new chunk when adding this block would push the current
        // one over budget — but never split off an empty chunk (an
        // oversized single block still goes on its own).
        if !current.is_empty() && current_chars + block_chars > max_chars {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current.push((block_id, content));
        current_chars += block_chars;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
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
        // Generous headroom (not just "enough for the JSON itself"):
        // reasoning-tuned local models (e.g. Qwen3.6) emit a `<think>...
        // </think>` block before the actual answer regardless of how
        // short the answer is — `strip_reasoning_block` below removes it
        // from the parsed response, but only once the model actually
        // finishes it within budget. A tight cap just truncates mid-think
        // with no JSON ever produced. Doesn't slow down non-reasoning
        // models: generation still stops at EOS, this is only a ceiling.
        max_tokens: Some(4096),
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

/// Fallback prompt used when [`PAGE_SUMMARY_PROMPT`]'s strict JSON output
/// can't be produced by the currently-loaded model — typically because
/// the chat template auto-injects an unclosed `<think>` block the model
/// never resolves, or the model is a creative-writing fine-tune that
/// won't respect a JSON schema. Asks for plain prose so *some* usable
/// summary lands in the UI instead of nothing.
const PLAIN_SUMMARY_PROMPT: &str = r##"You are a careful research assistant. You will be given a title and content (an article, transcript, or similar).

Write a short plain-text summary of the content in 3-6 sentences, in your own words, covering the main points and any distinct topics discussed. Do not use JSON, markdown, or bullet lists — just plain prose paragraphs.

Return ONLY the summary paragraphs, no preamble like "Here is a summary:" and no meta commentary."##;

fn summary_options() -> CompletionOptions {
    CompletionOptions {
        // See the comment on `concept_extraction_options` — same
        // reasoning-model headroom rationale, plus this prompt's answer
        // itself is naturally longer (multi-topic summaries).
        max_tokens: Some(4096),
        temperature: Some(0.3),
        ..Default::default()
    }
}

/// Parse LLM response into ConceptExtraction structs.
fn parse_concept_response(
    response: &str,
    original_content: &str,
) -> Result<Vec<ConceptExtraction>> {
    let trimmed = strip_reasoning_block(response.trim());
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

    let trimmed = strip_reasoning_block(response.trim());
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

/// Appends Qwen3's documented `/no_think` directive to a user-message
/// body so hybrid-reasoning models (Qwen3, Qwen3.5, Qwen3.6, ...) skip
/// their `<think>...</think>` block entirely for JSON-output calls where
/// we need the raw structured answer as fast as possible and there's no
/// point burning generation budget on chain-of-thought reasoning.
///
/// Why do it on the *input* side even though `strip_reasoning_block`
/// exists as belt-and-suspenders on the output side: with the older
/// `llama_chat_apply_template` C API our `llama-cpp-2` wraps we can't
/// pass an `enable_thinking: false` Jinja variable that newer chat
/// templates check for, so reasoning mode stays on by default. Once on,
/// Qwen3.6 in particular has been observed to (a) burn the entire
/// `max_tokens` budget on unfinished thinking and never emit JSON, or
/// (b) worse — get into a broken state where it opens `<think>` and
/// immediately emits EOS, returning a 7-character response with no
/// answer at all. Adding `/no_think` to the user message is Qwen's
/// documented soft switch to skip reasoning per turn, and is silently
/// ignored by non-Qwen3 models (harmless trailing text).
///
/// Per Qwen's docs, when both `/think` and `/no_think` appear the LAST
/// one wins — appending here guarantees `/no_think` prevails over any
/// user-content that happens to contain `/think`.
pub(crate) fn append_no_think_directive(user_content: &str) -> String {
    // Two newlines to guarantee it's a standalone line rather than
    // getting glued onto the tail of another sentence — this matters
    // because Qwen's `/no_think` parser is line-oriented.
    format!("{}\n\n/no_think", user_content.trim_end())
}

/// Strips a leading `<think>...</think>` (or `<thinking>...</thinking>`)
/// reasoning block some local models (e.g. Qwen3.6's hybrid reasoning mode)
/// emit before their actual answer — the chat-template plumbing in
/// `local_llm.rs` uses llama.cpp's older, template-string-only
/// `llama_chat_apply_template` API, which can't pass the
/// `enable_thinking: false` Jinja variable newer chat templates check for
/// to suppress this, so it has to be handled on the *output* side instead.
///
/// Kept as belt-and-suspenders even alongside
/// [`append_no_think_directive`]: some Qwen3 forks or non-Qwen reasoning
/// models don't honor `/no_think` but still emit reasoning tags, and we'd
/// rather strip successfully than parse-fail.
///
/// Only strips a *complete* block (both tags present) — if the model never
/// closed the tag (its whole response is the tag plus unfinished reasoning,
/// e.g. because `max_tokens` ran out mid-thought), there's no real answer
/// to recover here regardless, so this deliberately leaves that case alone
/// and lets the existing "missing JSON object/array" error surface as-is
/// (its snippet already makes an unclosed `<think>` block obvious).
pub(crate) fn strip_reasoning_block(response: &str) -> &str {
    let trimmed = response.trim_start();
    for (open, close) in [("<think>", "</think>"), ("<thinking>", "</thinking>")] {
        if let Some(rest) = trimmed.strip_prefix(open) {
            if let Some(end) = rest.find(close) {
                return rest[end + close.len()..].trim_start();
            }
        }
    }
    response
}

/// Rejects "responses" that technically aren't empty but are so short or
/// so obviously fragmentary that showing them as a summary is worse than
/// admitting the model failed. The bar we're calibrating against is
/// aggressively-quantized creative-writing fine-tunes (Fable-Fusion at
/// IQ2_M is the archetype) that respond to summarization prompts with
/// literally `Here` and then EOS, or `Sure, here's a summary:` and then
/// EOS, or just `<think>`. All of those are worse-than-no-summary from
/// a user perspective — they look valid to the JSON parser / the "not
/// empty" check but tell the reader nothing.
///
/// Threshold rationale: legitimate summaries in practice run to at
/// least a sentence (~40+ chars, 8+ words). We reject shorter than
/// **30 chars** OR fewer than **6 words** — that catches the observed
/// failure modes without rejecting a terse-but-real one-sentence
/// summary. The trade-off is: sometimes we throw away a slightly-too-
/// short real answer and cascade to the next retry tier, which is a
/// UX cost of a few extra seconds of retry; but the alternative is
/// showing `Here` as the summary, which is UX poison.
pub(crate) fn is_substantive_summary_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() < 30 {
        return false;
    }
    let word_count = trimmed.split_whitespace().count();
    if word_count < 6 {
        return false;
    }
    // Explicit reject-list for the "preamble-only" failure mode: models
    // that emitted an intro phrase and then EOS'd. Trailing punctuation
    // is stripped so `"Here is a summary:"` and `"Here is a summary"`
    // are both matched. Lowercased for case-insensitive comparison.
    let lower = trimmed
        .trim_end_matches(|c: char| c == ':' || c == '.' || c == ',' || c.is_whitespace())
        .to_ascii_lowercase();
    const PREAMBLE_ONLY: &[&str] = &[
        "here",
        "here is",
        "here is a summary",
        "here's a summary",
        "here is the summary",
        "sure",
        "sure, here",
        "sure, here is",
        "sure, here's a summary",
        "okay",
        "ok",
        "certainly",
        "of course",
        "the following",
        "below is a summary",
    ];
    if PREAMBLE_ONLY.contains(&lower.as_str()) {
        return false;
    }
    true
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

    let trimmed = strip_reasoning_block(response.trim());

    // Fallback path: if the model produced free-form prose instead of the
    // JSON envelope we asked for (common on aggressively-quantized or
    // creative-writing fine-tuned models that struggle with strict
    // instruction-following), still surface *something* usable to the
    // user instead of hiding the summary entirely. Try JSON first; on any
    // failure at all (no `{...}` found, malformed JSON, empty `topics`),
    // fall back to wrapping the raw model output as a single "Summary"
    // topic. Users can then see the summary and know it's what the model
    // wrote, rather than the whole button silently producing no output.
    let structured = extract_json_object(trimmed)
        .ok()
        .and_then(|json_str| serde_json::from_str::<SummaryJson>(json_str).ok());

    if let Some(parsed) = structured {
        let topics: Vec<TopicSummary> = parsed
            .topics
            .into_iter()
            .filter(|topic| is_substantive_summary_text(&topic.summary))
            .map(|topic| TopicSummary {
                topic: topic.topic.trim().to_string(),
                summary: topic.summary.trim().to_string(),
                tags: clean_tag_terms(topic.tags),
            })
            .collect();

        // Only accept the JSON-envelope answer if it has at least one
        // substantive topic OR a substantive title_answer. A bare
        // `title_answer: "Here"` with no topics counts as a failure and
        // cascades to the plain-text retry, same as if the JSON hadn't
        // parsed at all — the whole point is "did we get a real
        // summary?", not "did the JSON schema parse?".
        let title_ok = parsed
            .title_answer
            .as_deref()
            .is_some_and(is_substantive_summary_text);
        if !topics.is_empty() || title_ok {
            return Ok(PageSummary {
                title_answer: parsed
                    .title_answer
                    .filter(|answer| !answer.trim().is_empty()),
                topics,
            });
        }
    }

    // Free-text fallback: use whatever the model produced verbatim. Better
    // to surface a plain summary than to fail the whole button because the
    // model didn't wrap it in the exact JSON scaffolding we asked for.
    //
    // Guard against the "broken reasoning tag" case first: some fine-tunes
    // (notably Fable-Fusion, an aggressively creative-writing Qwen3.6
    // fine-tune) have had their `<think>...</think>` reasoning training
    // damaged — they emit `<think>` at the very start of the response and
    // then hit end-of-sequence *without ever closing the tag or producing
    // any actual content*. `strip_reasoning_block` deliberately leaves an
    // unclosed tag untouched (see its rationale), so the response arrives
    // here as literally the 7-char string `<think>`. Showing that to the
    // user as their "summary" is worse than telling them what happened,
    // so surface a targeted, actionable error instead.
    let plain = trimmed.trim();
    let plain_no_open_think = plain
        .strip_prefix("<think>")
        .or_else(|| plain.strip_prefix("<thinking>"))
        .map(str::trim_start)
        .unwrap_or(plain);
    if plain_no_open_think.is_empty() {
        return Err(summary_parse_error(
            "the model produced no summary text (it emitted an unclosed <think> tag \
             and then stopped, which usually means the chosen model's reasoning-mode \
             training is broken — try a different chat model in Settings)",
            response,
        ));
    }
    if plain.is_empty() {
        return Err(summary_parse_error(
            "empty page summary response from the model",
            response,
        ));
    }
    // Reject substance-less "responses" the same way the JSON path
    // above does — a plain `Here` or `Sure, here's a summary:` slipping
    // through the parser and being shown to the user as their summary
    // is exactly the failure mode this whole three-tier retry cascade
    // exists to prevent. Cascading to plain-text / last-resort retry
    // instead is much better UX than displaying a one-word non-summary.
    if !is_substantive_summary_text(plain_no_open_think) {
        return Err(summary_parse_error(
            "the model returned a fragmentary/preamble-only response with no actual \
             summary content (e.g. \"Here\", \"Sure, here's a summary:\", or a bare \
             `<think>` tag). This usually means an aggressively-quantized \
             creative-writing fine-tune has damaged instruction-following — try a \
             different chat model in Settings",
            response,
        ));
    }
    Ok(PageSummary {
        title_answer: None,
        topics: vec![TopicSummary {
            topic: "Summary".to_string(),
            summary: plain_no_open_think.to_string(),
            tags: Vec::new(),
        }],
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

/// Same shape as [`concept_parse_error`] but with wording that matches
/// the summarization pipeline. Used so a user staring at "Couldn't
/// produce a summary" doesn't also see "concept extraction response",
/// which reads like an unrelated internal component leaked out.
pub(crate) fn summary_parse_error(reason: &str, response: &str) -> CoreError {
    CoreError::Parse(format!(
        "Failed to parse page summary response ({reason}). Response snippet: {}",
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

    #[test]
    fn strip_reasoning_block_removes_a_closed_think_tag() {
        let response = "<think>hmm, let me consider this</think>\n\n[{\"text\":\"Rust\"}]";

        assert_eq!(strip_reasoning_block(response), r#"[{"text":"Rust"}]"#);
    }

    #[test]
    fn strip_reasoning_block_leaves_unclosed_think_tag_alone() {
        // No closing `</think>` — e.g. the model ran out of `max_tokens`
        // mid-thought. Nothing usable to recover, so this must be a no-op
        // rather than e.g. stripping to an empty string.
        let response = "<think>hmm, let me consider this";

        assert_eq!(strip_reasoning_block(response), response);
    }

    #[test]
    fn strip_reasoning_block_is_a_no_op_without_a_think_tag() {
        let response = r#"[{"text":"Rust"}]"#;

        assert_eq!(strip_reasoning_block(response), response);
    }

    #[test]
    fn is_substantive_summary_text_rejects_the_one_word_here_response() {
        // The exact regression that motivated this validator: user
        // picked Fable-Fusion-711-IQ2_M, model returned literally
        // "Here" and hit EOS. Parser used to accept this as a valid
        // one-topic summary and show "Summary: Here" to the user,
        // which is worse than admitting the model failed. Reject it
        // so the parse cascade proceeds to the next retry tier.
        assert!(!is_substantive_summary_text("Here"));
        assert!(!is_substantive_summary_text("  Here  "));
    }

    #[test]
    fn is_substantive_summary_text_rejects_preamble_only_responses() {
        // Same failure mode with slightly different sampling: some
        // models emit an intro phrase and then stop. All of these are
        // worse-than-no-summary from the user's perspective.
        for preamble in [
            "Sure, here's a summary:",
            "Here is a summary.",
            "Certainly",
            "Of course",
            "OK",
        ] {
            assert!(
                !is_substantive_summary_text(preamble),
                "preamble-only response should be rejected: {preamble:?}"
            );
        }
    }

    #[test]
    fn is_substantive_summary_text_accepts_a_normal_one_sentence_summary() {
        // Baseline: a real (if terse) one-sentence summary must pass.
        // If the threshold ever gets so tight that this fails, we've
        // over-corrected and are throwing away legitimate model output.
        let real = "This article explains how magnesium supports sleep quality and mood.";
        assert!(is_substantive_summary_text(real));
    }

    #[test]
    fn is_substantive_summary_text_rejects_the_bare_think_tag() {
        // Even after `strip_reasoning_block` no-ops on an unclosed
        // `<think>` tag, the check must still reject it — the tag is
        // 7 chars, less than the 30-char threshold.
        assert!(!is_substantive_summary_text("<think>"));
    }

    #[test]
    fn parse_concept_response_handles_a_leading_think_block() {
        let response =
            "<think>the content is about rust and parsing</think>[{\"text\":\"Rust\",\"type\":\"concept\"}]";

        let concepts = parse_concept_response(response, "Rust makes parsing strict")
            .expect("concept response with a closed <think> block should parse");

        assert_eq!(concepts.len(), 1);
    }

    #[test]
    fn append_no_think_directive_puts_the_switch_on_its_own_line() {
        let out = append_no_think_directive("some user content");

        // Must end with `/no_think` on a standalone line — Qwen's parser
        // is line-oriented, so `... user content/no_think` would NOT be
        // recognized.
        assert!(out.ends_with("\n\n/no_think"));
        assert!(out.starts_with("some user content"));
    }

    #[test]
    fn append_no_think_directive_overrides_a_user_provided_think_switch() {
        // Qwen's docs say the LAST directive wins — appending
        // `/no_think` after user content that already contains `/think`
        // must still land the `/no_think` last so it prevails.
        let out = append_no_think_directive("please analyze this /think");

        assert!(out.ends_with("/no_think"));
        assert!(!out.ends_with("/think"));
    }

    #[test]
    fn append_no_think_directive_does_not_double_up_trailing_whitespace() {
        // A user body that already ends with newlines should still
        // produce exactly one blank line before the directive, not
        // three or four.
        let out = append_no_think_directive("user body\n\n\n");

        assert_eq!(out, "user body\n\n/no_think");
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
                &crate::cancel::CancellationToken::disabled(),
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

    #[test]
    fn chunk_blocks_by_content_size_keeps_short_pages_in_one_chunk() {
        // Two tiny blocks: their combined content is nowhere near the
        // budget, so they must stay in a single chunk (i.e. one LLM
        // call). This is what preserves the existing "single batched
        // call for a short page" fast path — the chunker must never
        // over-split.
        let blocks = vec![("b1", "hello world"), ("b2", "another short one")];
        let chunks = chunk_blocks_by_content_size(&blocks, 6000);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2);
    }

    #[test]
    fn chunk_blocks_by_content_size_splits_when_cumulative_content_would_overflow() {
        // Six 200-char blocks with a 500-char budget must land as three
        // pairs (each pair = 400 chars, next block would push to 600),
        // proving cumulative-size splitting is what actually decides
        // when a new chunk starts — not block count.
        let block_content = "x".repeat(200);
        let blocks: Vec<(&str, &str)> = (0..6).map(|_| ("b", block_content.as_str())).collect();

        let chunks = chunk_blocks_by_content_size(&blocks, 500);

        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() == 2));
    }

    #[test]
    fn chunk_blocks_by_content_size_keeps_oversized_block_on_its_own() {
        // A single block already bigger than the whole budget still has
        // to go through as one chunk-of-one — there is no smaller unit
        // to split it into, and dropping/truncating it would break the
        // "concept text must appear verbatim in that block's content"
        // contract downstream.
        let huge = "y".repeat(20_000);
        let blocks = vec![("small-a", "x"), ("huge", huge.as_str()), ("small-b", "z")];

        let chunks = chunk_blocks_by_content_size(&blocks, 500);

        // Expected shape: [small-a], [huge alone], [small-b].
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec!["small-a"]);
        assert_eq!(chunks[1].iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec!["huge"]);
        assert_eq!(chunks[2].iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec!["small-b"]);
    }

    #[tokio::test]
    async fn extract_concepts_batch_splits_a_long_transcript_into_multiple_llm_calls() -> Result<()> {
        // Simulate an imported YouTube transcript: many short blocks
        // whose combined content overflows the concept-extraction
        // budget. This is the whole reason the chunking exists —
        // without it, a single ~7-8k-token prompt forces llama.cpp to
        // reserve a compute buffer that big and the context creation
        // fails ("null reference from llama.cpp") on a VRAM-tight
        // Vulkan backend.
        //
        // Sized so we split into at least 2 batched calls (never a
        // trailing 1-block "single-block" call — that path uses a
        // different prompt and would complicate the "we made N
        // batched calls" assertion below without adding coverage).
        // 14 blocks * 800 chars = 11200 chars, budget 6000 → chunks
        // of 7 blocks each, 2 batched calls total.
        let block_content = "a".repeat(800);
        let blocks: Vec<(String, String)> = (0..14)
            .map(|i| (format!("block-{i}"), block_content.clone()))
            .collect();
        let blocks_refs: Vec<(&str, &str)> = blocks
            .iter()
            .map(|(id, content)| (id.as_str(), content.as_str()))
            .collect();

        // Two batched calls expected — queue two empty-concepts JSON
        // responses (valid for the batched schema). If chunking is
        // off, the test will fail on "No mock LLM response queued"
        // (only one response in the queue would be consumed).
        let (llm, llm_state) = MockLlm::new([
            "[]".to_string(),
            "[]".to_string(),
        ]);

        let engine = ReferenceEngine::new(ReferenceConfig::default());
        let out = engine
            .extract_concepts_batch(
                &blocks_refs,
                &llm,
                &mut |_| {},
                &crate::cancel::CancellationToken::disabled(),
            )
            .await?;

        assert_eq!(out.len(), 14);
        assert!(out.iter().all(|v| v.is_empty()));

        let state = llm_state.lock().unwrap();
        assert_eq!(
            state.calls, 2,
            "expected exactly two batched LLM calls for a long transcript, got {}",
            state.calls
        );
        // Every block id must appear in at least one batched user
        // message (the batched prompt serialises each block's id and
        // content into JSON, so the id shows up verbatim).
        for i in 0..14 {
            let id = format!("block-{i}");
            assert!(
                state.user_messages.iter().any(|m| m.contains(&id)),
                "{id} was not mentioned in any batched LLM call"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn generate_references_still_returns_summary_when_concept_extraction_fails(
    ) -> Result<()> {
        // The scenario this exists to guard against: a long YouTube
        // transcript triggers a real LLM/context-creation failure
        // during concept extraction (e.g. null-context OOM). Before
        // the resilience fix, that turned the whole "Analyze this
        // Page" into a bare error with no summary. Now the summary
        // path (which caps its own input at 8k chars and is safe on
        // its own) must still run.
        //
        // Use a single block so the single-block extraction path runs
        // and cleanly hits `parse_concept_response` failure on the
        // malformed first response — no fallback re-tries to reason
        // about, just: extraction returns Err → resilience catches →
        // summary path proceeds with the next queued response.
        let page_title = "Long Video";
        let blocks = vec![(
            "block-0".to_string(),
            "Block content long enough to be eligible for extraction.".to_string(),
        )];

        let (llm, _llm_state) = MockLlm::new([
            "not valid json at all".to_string(),
            r#"{"title_answer": "the video is about testing", "topics": [{"topic": "Testing", "summary": "It covers unit testing best practices.", "tags": [{"term": "testing"}]}]}"#
                .to_string(),
        ]);
        let (embedder, _es) = MockEmbedder::new(HashMap::new());
        let store = MockVectorStore::new(HashMap::new());

        let engine = ReferenceEngine::new(ReferenceConfig::default());
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
                &crate::cancel::CancellationToken::disabled(),
            )
            .await?;

        // Concept extraction failed → zero references, but summary
        // survived.
        assert_eq!(meta.reference_count, 0);
        let summary = meta
            .summary
            .expect("summary should still be produced even when concept extraction fails");
        assert_eq!(
            summary.title_answer.as_deref(),
            Some("the video is about testing")
        );
        assert_eq!(summary.topics.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn generate_page_summary_falls_back_to_last_resort_when_first_two_attempts_emit_only_think(
    ) -> Result<()> {
        // Regression for the "model refuses to summarize" case: a
        // reasoning-mode fine-tune ships a chat template that makes
        // it emit `<think>` and immediately hit EOS, twice in a row
        // (once for the structured JSON prompt, once for the plain
        // fallback). Before the third-tier retry, the whole button
        // failed even though a bare higher-temp prompt could still
        // salvage a summary. This test locks in that the third
        // attempt actually happens *and* its output gets used.
        let title = "Some page";
        let content = "Body content the model needs to summarize.";

        let (llm, llm_state) = MockLlm::new([
            "<think>".to_string(),
            "<think>".to_string(),
            "Last-resort prose summary that finally got produced.".to_string(),
        ]);

        let summary = generate_page_summary(
            title,
            content,
            &llm,
            &mut |_| {},
            &crate::cancel::CancellationToken::disabled(),
        )
        .await?;
        assert!(
            summary
                .topics
                .iter()
                .any(|t| t.summary.contains("Last-resort prose summary")),
            "expected the last-resort attempt's response to end up as the summary, \
             got: {:?}",
            summary.topics
        );
        assert_eq!(
            llm_state.lock().unwrap().calls,
            3,
            "all three LLM attempts (structured, plain, last-resort) should have run"
        );
        Ok(())
    }

    #[tokio::test]
    async fn generate_page_summary_returns_actionable_error_naming_model_when_all_attempts_empty(
    ) -> Result<()> {
        // If even the last-resort high-temperature retry comes back
        // empty, we're out of options — but the error surfaced to the
        // user must name *which* model is broken and point them at
        // Settings, not just say "try a different model". Otherwise
        // a user with multiple installed models has to guess.
        let title = "Some page";
        let content = "Body content the model utterly refuses to summarize.";

        let (llm, llm_state) = MockLlm::new([
            "<think>".to_string(),
            "<think>".to_string(),
            "<think>".to_string(),
        ]);

        let err = generate_page_summary(
            title,
            content,
            &llm,
            &mut |_| {},
            &crate::cancel::CancellationToken::disabled(),
        )
        .await
        .expect_err("all-empty responses should produce a parse error");
        assert_eq!(
            llm_state.lock().unwrap().calls,
            3,
            "even a fully-broken model should get all three attempts before we give up"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("mock-llm"),
            "error should name the currently loaded model so the user knows what to change, \
             got: {msg}"
        );
        assert!(
            msg.contains("Settings"),
            "error should point the user at Settings, got: {msg}"
        );
        Ok(())
    }
}
