//! Web Research: an agentic loop that turns a page/topic into a handful of
//! real internet searches, judges which results are worth reading, fetches
//! and reads them, then synthesizes a topic-by-topic summary with inline
//! `[n]` citation markers pointing at the actual source URLs.
//!
//! This is deliberately a separate feature from [`crate::ai::references`]'s
//! "Analyze this Page"/"Analyze Selection": those only ever summarize
//! content already present in the page/selection and never leave the local
//! graph, while this module actually queries the internet, so results can
//! (and will) include information not already in Grafium and need visible,
//! clickable sources for the user to verify before trusting them.
//!
//! Reuses rather than reimplements: [`crate::scraping::search::web_search`]
//! for the actual internet query (a plain search-results-page scrape, no
//! paid search API — see that module's docs), [`crate::scraping::extract`]
//! for turning a fetched page into readable text (the same machinery
//! [`crate::scraping::clipper`] uses), the [`crate::ai::traits::LlmProvider`]/
//! [`CompletionOptions`] abstraction used everywhere else in this crate,
//! [`crate::parser::TagTerm`] for tags (so terms found in cited sources get
//! the exact same disambiguation-aware `[[wiki-link]]` wrapping as local
//! summaries), and [`crate::ai::references::TagJson`]/[`clean_tag_terms`]
//! for tolerant tag-array JSON parsing. This file only adds the
//! orchestration and prompts unique to "search the web, then write a cited
//! summary."

use serde::{Deserialize, Serialize};

use crate::ai::reasoning::{strip_think_blocks, ThinkStripResult, REASONING_ONLY_MESSAGE};
use crate::ai::references::{
    append_no_think_directive, clean_tag_terms, concept_parse_error, extract_json_object,
    research_parse_error, TagJson,
};
use crate::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};
use crate::parser::TagTerm;
use crate::scraping::browser::BrowserDriver;
use crate::scraping::extract;
use crate::scraping::search::{web_search, SearchResult as WebSearchResult};

/// Error message returned when a research run is stopped via its cancellation
/// flag. Callers that own the flag can compare against this (or just re-check
/// the flag) to tell a deliberate Stop apart from a genuine failure, so a
/// cancelled run doesn't surface an alarming "research failed" note the user
/// never asked to see.
pub const RESEARCH_CANCELLED: &str = "Web research was cancelled.";

const SYNTHESIS_MAX_TOKENS: u32 = 4096;
const SYNTHESIS_FALLBACK_MAX_TOKENS: u32 = 1800;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TopicJson {
    #[serde(default)]
    topic: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: Vec<TagJson>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SynthesisJson {
    #[serde(default)]
    title_answer: Option<String>,
    #[serde(default)]
    topics: Vec<TopicJson>,
}

/// Whether a (borrowed) cancellation flag has been tripped. `None` means "no
/// flag supplied" and so is never cancelled — the uncancellable callers.
pub(crate) fn is_cancelled(cancel: Option<&std::sync::atomic::AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
}

/// The canonical cancellation error (see [`RESEARCH_CANCELLED`]).
pub(crate) fn cancelled_error() -> CoreError {
    CoreError::Other(RESEARCH_CANCELLED.to_string())
}

/// A single web source cited by the research summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    /// 1-based citation number, referenced inline in `ResearchTopic::summary`
    /// as e.g. `"[2]"` — matches this citation's position in
    /// [`WebResearchResult::citations`].
    pub number: usize,
    pub title: String,
    pub url: String,
}

/// One topic's cited summary paragraph, mirroring
/// [`crate::ai::references::TopicSummary`] but with inline `[n]` citation
/// markers in `summary` pointing into the parent
/// [`WebResearchResult::citations`] list, since (unlike a local-only
/// summary) claims here come from external sources the user hasn't
/// necessarily read and may want to verify.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchTopic {
    pub topic: String,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<TagTerm>,
}

/// The result of a full web research run: an optional direct answer to the
/// page's title (if it posed a question), one cited paragraph per distinct
/// topic found across the fetched sources, and the flat numbered source
/// list those `[n]` markers refer to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebResearchResult {
    pub title_answer: Option<String>,
    pub topics: Vec<ResearchTopic>,
    /// Sources actually fetched and cited, in `[n]` order (1-based).
    pub citations: Vec<Citation>,
}

/// Tunables for a research run. Defaults are chosen to keep a run to a
/// handful of fetches (a few seconds to low tens of seconds on a local
/// model + home internet connection) rather than an open-ended crawl.
#[derive(Debug, Clone)]
pub struct WebResearchConfig {
    /// How many distinct search queries the LLM may generate from the seed
    /// topic/text.
    pub max_queries: usize,
    /// How many raw results to request per query from the search engine.
    pub results_per_query: usize,
    /// How many of the candidate results (across all queries, after
    /// dedup by URL) the LLM may pick to actually fetch and read.
    pub max_sources: usize,
}

impl Default for WebResearchConfig {
    fn default() -> Self {
        Self {
            max_queries: 3,
            results_per_query: 5,
            max_sources: 6,
        }
    }
}

/// Orchestrates one web research run: plan queries → search → pick sources
/// → fetch → synthesize a cited summary.
pub struct WebResearchEngine<'a> {
    llm: &'a dyn LlmProvider,
    browser: &'a dyn BrowserDriver,
    config: WebResearchConfig,
}

impl<'a> WebResearchEngine<'a> {
    pub fn new(llm: &'a dyn LlmProvider, browser: &'a dyn BrowserDriver) -> Self {
        Self {
            llm,
            browser,
            config: WebResearchConfig::default(),
        }
    }

    pub fn with_config(mut self, config: WebResearchConfig) -> Self {
        self.config = config;
        self
    }

    /// Researches `title`/`seed_text` (a page's title + content, or a
    /// selection) on the open internet, reporting each step through
    /// `progress` so a caller can show a live "what am I doing" status
    /// (planning queries, searching, reading source N/M, synthesizing)
    /// instead of a silent multi-second-to-minute wait.
    ///
    /// This is the uncancellable convenience form; long-running callers that
    /// need a Stop button should use [`Self::research_cancellable`].
    pub async fn research(
        &self,
        title: &str,
        seed_text: &str,
        progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<WebResearchResult> {
        self.research_cancellable(title, seed_text, None, progress)
            .await
    }

    /// [`Self::research`] with cooperative cancellation. `cancel` (if given) is
    /// polled between the expensive network steps — before each search query
    /// and before reading each source — so a user who hits Stop actually
    /// interrupts a slow run instead of waiting out every remaining fetch. On
    /// cancellation the run returns [`RESEARCH_CANCELLED`] as an error rather
    /// than a partial (and therefore misleading, half-cited) summary; the
    /// caller decides how to present that — typically by staying silent, since
    /// the user asked to stop.
    ///
    /// The flag is only checked at these boundaries, never mid-fetch: an
    /// individual `web_search`/`fetch` is already bounded by the browser's
    /// 30-second timeout, so the coarse-grained checks keep the worst-case
    /// "time to actually stop" to one in-flight request without threading a
    /// cancellation token through the whole scraping stack.
    pub async fn research_cancellable(
        &self,
        title: &str,
        seed_text: &str,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<WebResearchResult> {
        progress("Planning search queries...");
        let queries = self.plan_queries(title, seed_text).await?;
        if queries.is_empty() {
            return Err(CoreError::Other(
                "The AI didn't produce any search queries for this topic.".to_string(),
            ));
        }

        progress(&format!("Searching the web ({} queries)...", queries.len()));
        let mut candidates: Vec<WebSearchResult> = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();
        let mut search_errors = Vec::new();
        for query in &queries {
            if is_cancelled(cancel) {
                return Err(cancelled_error());
            }
            let results = match web_search(self.browser, query, self.config.results_per_query).await
            {
                Ok(results) => results,
                Err(err) => {
                    tracing::warn!(query, error = %err, "web search query failed");
                    search_errors.push(err.to_string());
                    continue;
                }
            };
            for result in results {
                if seen_urls.insert(result.url.clone()) {
                    candidates.push(result);
                }
            }
        }
        if candidates.is_empty() {
            return if search_errors.is_empty() {
                Err(CoreError::Other(
                    "No search results were found for this topic.".to_string(),
                ))
            } else {
                Err(CoreError::Other(format!(
                    "No search queries completed successfully: {}",
                    search_errors.join("; ")
                )))
            };
        }

        if filter_wrong_domain_candidates(&mut candidates, title, seed_text) {
            return Err(CoreError::Other(
                "The search results were for the wrong kind of bike (bicycles/e-bikes), but this \
                 question appears to be about motorcycles/MCs. I refused to synthesize an answer \
                 from wrong-domain sources."
                    .to_string(),
            ));
        }
        if looks_like_constrained_recommendation_question(title, seed_text) {
            rerank_constrained_recommendation_candidates(&mut candidates, title, seed_text);
        }

        progress("Choosing the most relevant sources...");
        let picked = self.pick_sources(title, seed_text, &candidates).await?;
        if picked.is_empty() {
            return Err(CoreError::Other(
                "The AI didn't judge any search result as relevant enough to read.".to_string(),
            ));
        }

        let mut citations = Vec::new();
        let mut source_excerpts = Vec::new();
        for (i, candidate) in picked.iter().enumerate() {
            if is_cancelled(cancel) {
                return Err(cancelled_error());
            }
            progress(&format!(
                "Reading source {}/{}: {}",
                i + 1,
                picked.len(),
                candidate.title
            ));
            let Ok(resource) = self.browser.fetch(&candidate.url).await else {
                continue; // skip unreachable sources rather than failing the whole run
            };
            let Ok(content) = extract::extract(&resource) else {
                continue;
            };
            let number = citations.len() + 1;
            citations.push(Citation {
                number,
                title: if content.title.trim().is_empty() {
                    candidate.title.clone()
                } else {
                    content.title.clone()
                },
                url: candidate.url.clone(),
            });
            source_excerpts.push((number, content.text));
        }
        if citations.is_empty() {
            return Err(CoreError::Other(
                "Could not fetch any of the search results found for this topic.".to_string(),
            ));
        }

        if is_cancelled(cancel) {
            return Err(cancelled_error());
        }
        progress("Synthesizing cited summary...");
        let (title_answer, topics) = self.synthesize(title, seed_text, &source_excerpts).await?;

        Ok(WebResearchResult {
            title_answer,
            topics,
            citations,
        })
    }

    async fn plan_queries(&self, title: &str, seed_text: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct QueriesJson {
            #[serde(default)]
            queries: Vec<String>,
        }

        let mut prompt = format!(
            "Title: {title}\n\nContent excerpt:\n{}\n\nSuggest up to {} distinct, well-formed web \
             search engine queries that would find good sources to research and fact-check the \
             subject(s) covered here. Prefer specific, targeted queries over broad/generic ones. \
             If the user is asking for recommendations, generate queries that surface concrete \
             candidate names/models, comparisons, owner reports, and tradeoffs — not just generic \
             background criteria. If the requested category appears to conflict with the user's \
             constraints, include at least one broader/better-fit alternative-category query instead \
             of searching only the user's category word. If a term is ambiguous, disambiguate it from the user's \
             constraints (for example, treat bike+MC+repair+fuel as motorcycle, not bicycle).",
            truncate(seed_text, 2000),
            self.config.max_queries,
        );
        if let Some(instruction) = research_domain_instruction(title, seed_text) {
            prompt.push_str("\n\n");
            prompt.push_str(instruction);
        }
        let messages = [ChatMessage {
            role: MessageRole::User,
            content: append_no_think_directive(&prompt),
        }];
        let options = CompletionOptions {
            max_tokens: Some(200),
            temperature: Some(0.2),
            system_prompt: Some(
                "Reply with ONLY a JSON object of the form {\"queries\": [\"...\", ...]} — no \
                 other text, no markdown fences."
                    .to_string(),
            ),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let cleaned = clean_structured_response(&raw)?;
        let json_str = extract_json_object(cleaned.trim())?;
        let parsed: QueriesJson = serde_json::from_str(json_str).map_err(|e| {
            concept_parse_error(&format!("invalid search-query JSON: {e}"), cleaned.trim())
        })?;

        Ok(normalize_research_queries(
            parsed
                .queries
                .into_iter()
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty())
                .collect(),
            title,
            seed_text,
            self.config.max_queries,
        ))
    }

    async fn pick_sources(
        &self,
        title: &str,
        seed_text: &str,
        candidates: &[WebSearchResult],
    ) -> Result<Vec<WebSearchResult>> {
        #[derive(Deserialize)]
        struct PicksJson {
            #[serde(default)]
            picks: Vec<usize>,
        }

        let mut prompt = format!("Title: {title}\n");
        if !seed_text.trim().is_empty() && seed_text.trim() != title.trim() {
            prompt.push_str(&format!(
                "\nOriginal content excerpt / constraints:\n{}\n",
                truncate(seed_text, 1500)
            ));
        }
        prompt.push_str("\nCandidate search results (index: title — url — snippet):\n");
        for (i, candidate) in candidates.iter().enumerate() {
            prompt.push_str(&format!(
                "{i}: {} — {} — {}\n",
                truncate(&candidate.title, 100),
                candidate.url,
                truncate(&candidate.snippet, 200),
            ));
        }
        prompt.push_str(&format!(
            "\nPick up to {} of the most relevant, credible, and diverse (avoid near-duplicate \
             sources) results to actually read in full.",
            self.config.max_sources
        ));
        if looks_like_constrained_recommendation_question(title, seed_text) {
            prompt.push_str(CONSTRAINED_RECOMMENDATION_SOURCE_PICK_GUIDANCE);
        }
        if let Some(instruction) = research_domain_instruction(title, seed_text) {
            prompt.push_str("\n\n");
            prompt.push_str(instruction);
        }

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: append_no_think_directive(&prompt),
        }];
        let options = CompletionOptions {
            max_tokens: Some(150),
            temperature: Some(0.0),
            system_prompt: Some(
                "Reply with ONLY a JSON object of the form {\"picks\": [<indices>]} — no other \
                 text, no markdown fences. For recommendation questions, prefer sources that name \
                 concrete options/models or compare options against the user's constraints, not \
                 generic category overviews."
                    .to_string(),
            ),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let cleaned = clean_structured_response(&raw)?;
        let json_str = extract_json_object(cleaned.trim())?;
        let parsed: PicksJson = serde_json::from_str(json_str).map_err(|e| {
            concept_parse_error(&format!("invalid source-pick JSON: {e}"), cleaned.trim())
        })?;

        let mut seen = std::collections::HashSet::new();
        Ok(parsed
            .picks
            .into_iter()
            .filter(|&i| seen.insert(i))
            .filter_map(|i| candidates.get(i).cloned())
            .take(self.config.max_sources)
            .collect())
    }

    async fn synthesize(
        &self,
        title: &str,
        seed_text: &str,
        source_excerpts: &[(usize, String)],
    ) -> Result<(Option<String>, Vec<ResearchTopic>)> {
        let mut prompt = format!("Title: {title}\n");
        if !seed_text.trim().is_empty() {
            prompt.push_str(&format!(
                "\nOriginal content excerpt (for context, not itself a source):\n{}\n",
                truncate(seed_text, 1500)
            ));
        }
        prompt.push_str("\nSources (numbered — cite these numbers as [n] in your summary):\n");
        for (number, text) in source_excerpts {
            prompt.push_str(&format!("[{number}]:\n{}\n\n", truncate(text, 3000)));
        }
        prompt.push_str(&format!(
            "\nLanguage instruction: {}\n",
            crate::ai::answer_language_rule_for_question(title)
        ));
        if looks_like_constrained_recommendation_question(title, seed_text) {
            prompt.push_str(CONSTRAINED_RECOMMENDATION_SYNTHESIS_GUIDANCE);
        }
        if let Some(instruction) = research_domain_instruction(title, seed_text) {
            prompt.push_str("\n\n");
            prompt.push_str(instruction);
        }

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: append_no_think_directive(&prompt),
        }];
        let options = CompletionOptions {
            max_tokens: Some(SYNTHESIS_MAX_TOKENS),
            temperature: Some(0.3),
            system_prompt: Some(format!(
                "{RESEARCH_SYNTHESIS_PROMPT}\n\n{}",
                crate::ai::ANSWER_LANGUAGE_RULE
            )),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let mut parsed: SynthesisJson = match parse_synthesis_response(&raw) {
            Ok(parsed) => parsed,
            Err(_first_error) => {
                let retry_prompt = format!(
                    "{prompt}\n\nYour previous response did not contain a valid JSON object. \
Return exactly one JSON object matching the requested schema. Do not include <think>, analysis, \
markdown, examples, separators, or any prose before or after the JSON."
                );
                let retry_messages = [ChatMessage {
                    role: MessageRole::User,
                    content: append_no_think_directive(&retry_prompt),
                }];
                let retry_options = CompletionOptions {
                    max_tokens: Some(SYNTHESIS_MAX_TOKENS),
                    temperature: Some(0.0),
                    system_prompt: Some(format!(
                        "{RESEARCH_SYNTHESIS_PROMPT}\n\n{}",
                        crate::ai::ANSWER_LANGUAGE_RULE
                    )),
                    stop: None,
                    cancel: None,
                };
                let retry_raw = self.llm.complete(&retry_messages, &retry_options).await?;
                match parse_synthesis_response(&retry_raw) {
                    Ok(parsed) => parsed,
                    Err(_retry_error) => {
                        return self
                            .synthesize_text_fallback(title, seed_text, source_excerpts)
                            .await;
                    }
                }
            }
        };

        if looks_like_constrained_recommendation_question(title, seed_text) {
            match self
                .audit_constrained_recommendation(title, seed_text, source_excerpts, &parsed)
                .await
            {
                Ok(audited) if synthesis_has_content(&audited) => parsed = audited,
                Ok(_) => tracing::warn!("constrained recommendation audit returned no answer"),
                Err(error) => {
                    tracing::warn!(%error, "constrained recommendation audit failed");
                }
            }
        }

        let topics = parsed
            .topics
            .into_iter()
            .filter(|topic| !topic.summary.trim().is_empty())
            .map(|topic| ResearchTopic {
                topic: topic.topic.trim().to_string(),
                summary: topic.summary.trim().to_string(),
                tags: clean_tag_terms(topic.tags),
            })
            .collect();

        Ok((
            parsed
                .title_answer
                .filter(|answer| !answer.trim().is_empty()),
            topics,
        ))
    }

    async fn synthesize_text_fallback(
        &self,
        title: &str,
        seed_text: &str,
        source_excerpts: &[(usize, String)],
    ) -> Result<(Option<String>, Vec<ResearchTopic>)> {
        let mut prompt = format!("Title: {title}\n");
        if !seed_text.trim().is_empty() {
            prompt.push_str(&format!(
                "\nOriginal content excerpt (for context, not itself a source):\n{}\n",
                truncate(seed_text, 1500)
            ));
        }
        prompt.push_str("\nSources (numbered — cite these numbers as [n] in your answer):\n");
        for (number, text) in source_excerpts {
            prompt.push_str(&format!("[{number}]:\n{}\n\n", truncate(text, 3000)));
        }
        prompt.push_str(
            "\nThe structured JSON answer was invalid. Write the answer as concise Markdown \
instead. Use ONLY the numbered sources above, and put an inline citation like [1] or [2][4] \
after every factual claim. Do not write JSON, markdown fences, chain-of-thought, or any note \
about the failed JSON.",
        );
        if looks_like_constrained_recommendation_question(title, seed_text) {
            prompt.push_str(CONSTRAINED_RECOMMENDATION_SYNTHESIS_GUIDANCE);
        }
        if let Some(instruction) = research_domain_instruction(title, seed_text) {
            prompt.push_str("\n\n");
            prompt.push_str(instruction);
        }
        prompt.push_str(&format!(
            "\n\nLanguage instruction: {}\n",
            crate::ai::answer_language_rule_for_question(title)
        ));

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: append_no_think_directive(&prompt),
        }];
        let options = CompletionOptions {
            max_tokens: Some(SYNTHESIS_FALLBACK_MAX_TOKENS),
            temperature: Some(0.2),
            system_prompt: Some(format!(
                "You are a careful research assistant writing a cited answer from real, numbered \
web sources. Return only the user-facing answer text.\n\n{}",
                crate::ai::ANSWER_LANGUAGE_RULE
            )),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let answer = match strip_think_blocks(&raw) {
            ThinkStripResult::Answer(text) => text.trim().to_string(),
            ThinkStripResult::ReasoningOnly => String::new(),
        };
        if answer.is_empty() {
            return Err(CoreError::Parse(
                "research synthesis JSON failed and fallback answer was empty".to_string(),
            ));
        }

        Ok((
            None,
            vec![ResearchTopic {
                topic: "Web research".to_string(),
                summary: answer,
                tags: Vec::new(),
            }],
        ))
    }

    async fn audit_constrained_recommendation(
        &self,
        title: &str,
        seed_text: &str,
        source_excerpts: &[(usize, String)],
        draft: &SynthesisJson,
    ) -> Result<SynthesisJson> {
        let mut prompt = format!("Title: {title}\n");
        if !seed_text.trim().is_empty() {
            prompt.push_str(&format!(
                "\nOriginal content excerpt / constraints:\n{}\n",
                truncate(seed_text, 1500)
            ));
        }
        prompt.push_str("\nSources (numbered — cite these numbers as [n] in your answer):\n");
        for (number, text) in source_excerpts {
            prompt.push_str(&format!("[{number}]:\n{}\n\n", truncate(text, 3000)));
        }
        let draft_json = serde_json::to_string(draft).map_err(|error| {
            CoreError::Parse(format!(
                "failed to serialize research synthesis draft: {error}"
            ))
        })?;
        prompt.push_str(&format!(
            "\nDraft answer JSON to audit:\n{draft_json}\n\nReturn the corrected JSON object now."
        ));
        prompt.push_str(&format!(
            "\n\nLanguage instruction: {}\n",
            crate::ai::answer_language_rule_for_question(title)
        ));
        if let Some(instruction) = research_domain_instruction(title, seed_text) {
            prompt.push_str("\n\n");
            prompt.push_str(instruction);
        }

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: append_no_think_directive(&prompt),
        }];
        let options = CompletionOptions {
            max_tokens: Some(SYNTHESIS_MAX_TOKENS),
            temperature: Some(0.0),
            system_prompt: Some(format!(
                "{CONSTRAINED_RECOMMENDATION_AUDIT_PROMPT}\n\n{}",
                crate::ai::ANSWER_LANGUAGE_RULE
            )),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        parse_synthesis_response(&raw)
    }
}

fn clean_structured_response(raw: &str) -> Result<String> {
    match strip_think_blocks(raw) {
        ThinkStripResult::Answer(text) => Ok(text),
        ThinkStripResult::ReasoningOnly => Err(concept_parse_error(
            &format!("missing JSON object in response; {REASONING_ONLY_MESSAGE}"),
            "[reasoning-only response hidden]",
        )),
    }
}

fn parse_synthesis_response<T>(raw: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let cleaned = clean_structured_response(raw)?;
    let trimmed = cleaned.trim();
    let json_str = extract_json_object(trimmed)
        .map_err(|_| research_parse_error("missing or unterminated synthesis JSON", trimmed))?;
    serde_json::from_str(json_str)
        .map_err(|error| research_parse_error(&format!("invalid synthesis JSON: {error}"), trimmed))
}

fn synthesis_has_content(synthesis: &SynthesisJson) -> bool {
    synthesis
        .title_answer
        .as_deref()
        .is_some_and(|answer| !answer.trim().is_empty())
        || synthesis
            .topics
            .iter()
            .any(|topic| !topic.summary.trim().is_empty())
}

fn looks_like_constrained_recommendation_question(title: &str, seed_text: &str) -> bool {
    let combined = format!("{title}\n{seed_text}").to_lowercase();
    looks_like_recommendation_request(&combined) && looks_like_hard_constraint_request(&combined)
}

pub(crate) fn research_domain_instruction(title: &str, seed_text: &str) -> Option<&'static str> {
    if looks_like_motorcycle_research_question(title, seed_text) {
        Some(MOTORCYCLE_DOMAIN_INSTRUCTION)
    } else {
        None
    }
}

pub(crate) fn normalize_research_queries(
    queries: Vec<String>,
    title: &str,
    seed_text: &str,
    max_queries: usize,
) -> Vec<String> {
    let mut normalized = Vec::new();
    if looks_like_motorcycle_research_question(title, seed_text) {
        if looks_like_constrained_recommendation_question(title, seed_text) {
            normalized.push(
                "simple dual sport adventure motorcycle easy to repair reliable off road travel luggage fuel tank"
                    .to_string(),
            );
            normalized.push(
                "DR650 KLR650 XR650L Tenere 700 Himalayan comparison reliable off road travel motorcycle"
                    .to_string(),
            );
        }
        normalized.extend(queries.into_iter().map(|query| {
            if contains_motorcycle_domain_marker(&query.to_lowercase()) {
                query
            } else {
                format!("motorcycle {query}")
            }
        }));
    } else {
        normalized = queries;
    }

    let mut seen = std::collections::HashSet::new();
    normalized
        .into_iter()
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.to_lowercase()))
        .take(max_queries)
        .collect()
}

pub(crate) fn filter_wrong_domain_candidates(
    candidates: &mut Vec<WebSearchResult>,
    title: &str,
    seed_text: &str,
) -> bool {
    if !looks_like_motorcycle_research_question(title, seed_text) {
        return false;
    }

    let before = candidates.len();
    candidates.retain(motorcycle_candidate_is_not_obviously_wrong_domain);
    before > 0 && candidates.is_empty()
}

pub(crate) fn rerank_research_candidates(
    candidates: &mut [WebSearchResult],
    title: &str,
    seed_text: &str,
) {
    if looks_like_constrained_recommendation_question(title, seed_text) {
        rerank_constrained_recommendation_candidates(candidates, title, seed_text);
    }
}

fn looks_like_motorcycle_research_question(title: &str, seed_text: &str) -> bool {
    let text = format!("{title}\n{seed_text}").to_lowercase();
    contains_motorcycle_domain_marker(&text)
        || ((text.contains("bike") || text.contains("bikes"))
            && (text.contains("aux gas")
                || text.contains("gas tank")
                || text.contains("fuel tank")
                || text.contains("right to repair")
                || text.contains("wabdr")
                || text.contains("mc")
                || text.contains("river crossing")))
}

fn contains_motorcycle_domain_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "motorcycle",
        "motorcycles",
        "motorbike",
        "motorbikes",
        "dual sport",
        "dual-sport",
        "adventure bike",
        "dirt bike",
        "m/c",
        "harley",
        "indian",
        "gold wing",
        "klr",
        "dr650",
        "xr650",
        "tenere",
        "ténéré",
        "himalayan",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || contains_word(&lower, "mc")
}

fn motorcycle_candidate_is_not_obviously_wrong_domain(candidate: &WebSearchResult) -> bool {
    let haystack = format!("{} {}", candidate.title, candidate.snippet).to_lowercase();
    if contains_motorcycle_domain_marker(&haystack) || contains_motor_vehicle_marker(&haystack) {
        return true;
    }
    !contains_bicycle_domain_marker(&haystack)
}

fn contains_motor_vehicle_marker(text: &str) -> bool {
    [
        "fuel tank",
        "gas tank",
        "crash bars",
        "royal enfield",
        "moto guzzi",
    ]
    .iter()
    .any(|marker| text.contains(marker))
        || [
            "cc",
            "efi",
            "abs",
            "engine",
            "suzuki",
            "honda",
            "kawasaki",
            "yamaha",
            "bmw",
            "ktm",
            "husqvarna",
            "ducati",
            "triumph",
        ]
        .iter()
        .any(|marker| contains_word(text, marker))
}

fn contains_bicycle_domain_marker(text: &str) -> bool {
    [
        "bicycle",
        "bicycles",
        "cycling",
        "cyclist",
        "pedal",
        "pedals",
        "e-bike",
        "ebike",
        "electric bike",
        "road bike",
        "gravel bike",
        "mountain bike",
        "hybrid bike",
        "beach cruiser",
        "cruiser bicycle",
        "sixthreezero",
        "bicycling.com",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn contains_word(text: &str, word: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == word)
}

fn looks_like_recommendation_request(text: &str) -> bool {
    text.contains("recommend")
        || text.contains("what should i buy")
        || text.contains("what should i choose")
        || text.contains("what should i use")
        || text.contains("what is a good")
        || text.contains("what's a good")
        || text.contains("best ")
        || text.contains("top ")
        || (text.contains("which ") && (text.contains(" should i ") || text.contains(" to buy")))
}

fn looks_like_hard_constraint_request(text: &str) -> bool {
    if text.contains("constraint") || text.contains("criteria") || text.contains("requirement") {
        return true;
    }

    let markers = [
        "must",
        "need",
        "want",
        "right to repair",
        "easy to fix",
        "easy to repair",
        "reliable",
        "offroad",
        "off-road",
        "gravel",
        "poor maintained",
        "river crossing",
        "modifiable",
        "mod it",
        "luggage",
        "fuel tank",
    ];
    markers
        .iter()
        .filter(|marker| text.contains(**marker))
        .take(2)
        .count()
        >= 2
}

fn rerank_constrained_recommendation_candidates(
    candidates: &mut [WebSearchResult],
    title: &str,
    seed_text: &str,
) {
    let terms = weighted_constraint_terms(title, seed_text);
    if terms.is_empty() {
        return;
    }
    candidates.sort_by_cached_key(|candidate| {
        std::cmp::Reverse(constrained_recommendation_score(candidate, &terms))
    });
}

fn constrained_recommendation_score(
    candidate: &WebSearchResult,
    terms: &[(String, usize)],
) -> usize {
    let haystack = format!("{} {}", candidate.title, candidate.snippet).to_lowercase();
    terms
        .iter()
        .filter(|(term, _)| haystack.contains(term.as_str()))
        .map(|(_, weight)| *weight)
        .sum()
}

fn weighted_constraint_terms(title: &str, seed_text: &str) -> Vec<(String, usize)> {
    let combined = format!("{title}\n{seed_text}").to_lowercase();
    let mut terms = Vec::new();
    for phrase in [
        "right to repair",
        "easy to fix",
        "easy to repair",
        "off-road",
        "poor maintained",
        "river crossing",
        "fuel tank",
        "aux gas",
        "luggage carrier",
    ] {
        if combined.contains(phrase) {
            terms.push((phrase.to_string(), 6));
        }
    }

    let mut seen = std::collections::HashSet::new();
    for token in combined.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let token = token.trim();
        if token.len() < 3 || !seen.insert(token.to_string()) || is_query_stopword(token) {
            continue;
        }
        let weight = match token {
            "repair" | "fix" | "reliable" | "simple" | "offroad" | "gravel" | "forest"
            | "river" | "crossing" | "wabdr" | "mod" | "mods" | "luggage" | "fuel" | "tank"
            | "tanks" | "carriers" | "comfortable" | "comfort" => 4,
            _ => 1,
        };
        terms.push((token.to_string(), weight));
    }
    terms
}

fn is_query_stopword(token: &str) -> bool {
    matches!(
        token,
        "about"
            | "also"
            | "and"
            | "any"
            | "are"
            | "bike"
            | "bikes"
            | "buy"
            | "can"
            | "cruiser"
            | "for"
            | "from"
            | "get"
            | "give"
            | "good"
            | "have"
            | "here"
            | "how"
            | "into"
            | "like"
            | "list"
            | "look"
            | "model"
            | "models"
            | "motorcycle"
            | "motorcycles"
            | "need"
            | "not"
            | "one"
            | "please"
            | "research"
            | "should"
            | "some"
            | "that"
            | "the"
            | "this"
            | "top"
            | "use"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
            | "want"
            | "you"
            | "your"
    )
}

const RESEARCH_SYNTHESIS_PROMPT: &str = r##"You are a careful research assistant fact-checking and synthesizing information gathered from real, numbered web sources.

Your first priority is to answer the user's actual question, not to describe how one might answer it. If the user asks for recommendations, options, products, candidates, or "what should I buy/choose/use", give concrete named recommendations ranked by fit. Include a best overall pick when the sources support one, explain why it matches the constraints, and name important tradeoffs. Do not answer recommendation questions with only criteria, background, a category tour, or a restatement of the constraints.

If the user's requested category conflicts with their constraints, say so directly and recommend the closest better-fit category or options supported by the sources. If no source supports a perfect match, give the closest matches and explain what each compromises.

When the user gives constraints, treat them as a mandatory scoring rubric, not decorative preferences. Do not recommend a candidate unless the sources explicitly support its fit for the relevant hard constraints. If support is missing or contradicted, say the candidate is unsupported or a poor fit instead of papering over the gap. Do not infer off-road ability, field repairability, right-to-repair, mod support, or long-trip suitability merely from power, price, luxury, touring comfort, or brand reputation.

Do the research work for the user: synthesize the sources into an answer. Do NOT tell the user to "look at", "check", "consider researching", or "consult" categories/sources instead of answering. Source citations are evidence for your answer, not assignments for the user.

The sources may cover a single subject or several related subjects. Identify every distinct answer item worth reporting on, and write ONLY claims that are actually supported by the numbered sources provided — do not use outside knowledge, and do not invent facts.

For source-dependent practical procedures where exact buying advice, compatibility, product specs, material limits, current facts, or safety constraints matter, name the exact item/spec/process only when a numbered source supports it. Include compatibility and safety constraints found in the sources, explicitly say when the sources are too weak to support a safe procedure, and do not turn generic knowledge into a precise buying list or step-by-step process.

Do not write chain-of-thought, analysis, examples, markdown fences, separators, or prose outside the JSON. If your chat template supports thinking controls, treat this as /no_think.

Return a JSON object with:
- "title_answer": if the title poses a question, asks for a recommendation, or makes a claim the sources answer/support/refute, one sentence directly answering it, with an inline [n] citation. For recommendation questions, name the best overall pick here. Otherwise null.
- "topics": an array with one object per distinct topic, each with:
  - "topic": a short label for this specific subject. For recommendation questions, make each topic a named option such as "Best overall: Model X" or "Budget alternative: Model Y".
  - "summary": a 2-5 sentence paragraph synthesizing what the sources say about this topic. For recommendations, state what the option is, why it fits the user's constraints, how it compares to the other named options when the sources allow comparison, and the key compromise. EVERY factual claim must end with an inline citation marker like "[1]" or "[2][4]" pointing at the source number(s) that support it. If sources disagree, say so explicitly (e.g. "one source claims X[1], while another found no such effect[3]").
  - "tags": an array of 1-4 key term objects, each {"term": "...", "qualified": "..." (optional, only for ambiguous bare terms)} — same rules as regular page tagging: prefer a specific verbatim phrase, only add "qualified" when a short generic word would otherwise be ambiguous.

Example: {"title_answer": "Yes, magnesium supplementation shows a modest benefit for sleep onset[1][2].", "topics": [{"topic": "Magnesium and sleep", "summary": "Multiple sources report magnesium glycinate improves sleep onset latency[1], though effect sizes were small in a controlled trial[2]. One source notes benefits may be limited to people who are already magnesium-deficient[3].", "tags": [{"term": "magnesium"}, {"term": "sleep onset"}]}]}

Return ONLY the JSON object, no other text."##;

const CONSTRAINED_RECOMMENDATION_SOURCE_PICK_GUIDANCE: &str = "\n\nFor this constrained \
recommendation request, treat the user's constraints as mandatory relevance filters. Results that \
only match the requested category label but ignore or contradict the hard constraints are low \
relevance. Prefer comparison/owner/spec sources that can prove fit or tradeoffs across the \
constraints; do not pick a luxury/touring/category list merely because it names popular products.";

const CONSTRAINED_RECOMMENDATION_SYNTHESIS_GUIDANCE: &str = "\n\nConstrained recommendation rule: \
treat every explicit user requirement as part of the scoring rubric. Mandatory constraints outrank \
the category word the user used. A source about power, comfort, luxury, or road touring does not by \
itself support off-road ability, field repairability, right-to-repair, mod support, or travel \
durability. If the sources do not explicitly support a candidate on the relevant hard constraints, \
do not recommend it; call it unsupported/poor-fit or replace it with a better-supported option.";

const CONSTRAINED_RECOMMENDATION_AUDIT_PROMPT: &str = r##"You are auditing a cited recommendation answer against the user's explicit constraints and the numbered sources.

Use the same JSON schema as the draft: {"title_answer": string|null, "topics": [{"topic": string, "summary": string, "tags": [{"term": string}]}]}.

Rules:
- Treat every explicit user requirement as a scoring constraint. Mandatory constraints outrank the category word the user used.
- Keep a recommendation only if the numbered sources explicitly support its fit for the relevant constraints.
- Never infer off-road ability, field repairability, right-to-repair, mod support, or travel durability merely from power, price, luxury, touring comfort, or brand reputation.
- If the draft recommends a candidate that is unsupported or contradicted by the sources, remove it, downgrade it as a poor fit, or replace it with a better-supported candidate from the same sources.
- If the sources read are too weak or irrelevant to support any real recommendation, say that directly instead of pretending a bad candidate fits.
- Every factual claim in the corrected answer must end with an inline citation like [1] or [2][4].

Return ONLY the corrected JSON object, no markdown fences and no prose outside JSON."##;

const MOTORCYCLE_DOMAIN_INSTRUCTION: &str = "Domain instruction: This question is about \
motorcycles/MCs, not bicycles, e-bikes, cycling, or pedal cruiser bikes. Interpret words like \
\"bike\" and \"cruiser bike\" as motorcycle terms because the question mentions motor-vehicle \
constraints such as gas/fuel tanks, MC repair, right-to-repair computers, WABDR/off-road travel, \
or field repair. Reject bicycle/e-bike/cycling sources as wrong-domain evidence.";

pub(crate) fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_util::BoxFuture;
    use crate::scraping::browser::FetchedResource;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A stub `LlmProvider` that replies with the next response in a fixed
    /// queue, regardless of the prompt — enough to unit-test the
    /// plan/pick/synthesize pipeline without a real model.
    pub(crate) struct StubLlm {
        responses: Mutex<std::collections::VecDeque<String>>,
    }

    impl StubLlm {
        pub(crate) fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(String::from).collect()),
            }
        }
    }

    impl LlmProvider for StubLlm {
        fn complete<'a>(
            &'a self,
            _messages: &'a [ChatMessage],
            _options: &'a CompletionOptions,
        ) -> BoxFuture<'a, Result<String>> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "stub"
        }

        fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
            Box::pin(async move { Ok(true) })
        }
    }

    /// A stub `BrowserDriver` that serves canned Brave search-results HTML
    /// for any `search.brave.com` URL, and canned page HTML for everything
    /// else — enough to drive the whole `research()` pipeline in-process.
    struct StubBrowser {
        search_html: String,
        pages: HashMap<String, String>,
    }

    impl BrowserDriver for StubBrowser {
        fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
            Box::pin(async move {
                // Both engines the search layer may consult — it falls back
                // to DuckDuckGo when Brave rate-limits — are served the same
                // canned results page.
                let bytes = if url.starts_with("https://search.brave.com/")
                    || url.starts_with("https://html.duckduckgo.com/")
                {
                    self.search_html.clone().into_bytes()
                } else {
                    self.pages
                        .get(url)
                        .cloned()
                        .ok_or_else(|| CoreError::Other(format!("no stub page for {url}")))?
                        .into_bytes()
                };
                Ok(FetchedResource {
                    url: url.to_string(),
                    content_type: Some("text/html".to_string()),
                    bytes,
                })
            })
        }
    }

    fn search_html_with_two_results() -> String {
        search_html_with_results(&[
            (
                "https://a.example/article",
                "Source A",
                "A snippet about the topic.",
            ),
            ("https://b.example/article", "Source B", "Another snippet."),
        ])
    }

    fn search_html_with_results(results: &[(&str, &str, &str)]) -> String {
        let mut html = "<html><body>\n".to_string();
        for (url, title, snippet) in results {
            html.push_str(&format!(
                r#"<div class="snippet" data-type="web">
          <a href="{url}"><div class="title">{title}</div></a>
          <div class="generic-snippet"><div class="content">{snippet}</div></div>
        </div>
"#
            ));
        }
        html.push_str("</body></html>");
        html
    }

    fn page_html(title: &str, body: &str) -> String {
        format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
    }

    #[test]
    fn synthesis_prompt_requires_concrete_recommendation_answers() {
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("concrete named recommendations"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("best overall pick"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("Do NOT tell the user"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("not assignments for the user"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("category conflicts"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("mandatory scoring rubric"));
        assert!(RESEARCH_SYNTHESIS_PROMPT.contains("Do not infer off-road ability"));
        assert!(CONSTRAINED_RECOMMENDATION_AUDIT_PROMPT.contains("unsupported or contradicted"));
    }

    #[test]
    fn constrained_recommendation_detector_matches_hard_constraint_purchase_asks() {
        let motorcycle_question = "what is a good cruiser bike that is easy to fix and reliable and can also go offroad? I need right to repair, gravel and forest roads, luggage, aux gas tanks, and comfort.";
        assert!(looks_like_constrained_recommendation_question(
            motorcycle_question,
            motorcycle_question
        ));

        let ordinary_question = "what is a good way to fix a rust compiler error?";
        assert!(!looks_like_constrained_recommendation_question(
            ordinary_question,
            ordinary_question
        ));
    }

    #[tokio::test]
    async fn constrained_recommendations_prioritize_constraint_matching_sources() {
        let question = "what is a good cruiser bike that is easy to fix and reliable and can also go offroad? I need right to repair, gravel and forest roads, luggage, aux gas tanks, and comfort.";
        let browser = StubBrowser {
            search_html: search_html_with_results(&[
                (
                    "https://bad.example/gold-wing",
                    "Best touring motorcycles",
                    "Honda Gold Wing luxury comfort power road touring.",
                ),
                (
                    "https://good.example/dual-sports",
                    "DR650 KLR650 XR650L WABDR dual sport comparison",
                    "Simple reliable off-road gravel motorcycles with easy repair, aftermarket fuel tanks, luggage racks, and camping travel mods.",
                ),
            ]),
            pages: HashMap::from([
                (
                    "https://bad.example/gold-wing".to_string(),
                    page_html(
                        "Gold Wing",
                        "<p>The Honda Gold Wing is a luxury touring motorcycle with road comfort.</p>",
                    ),
                ),
                (
                    "https://good.example/dual-sports".to_string(),
                    page_html(
                        "Simple dual sports",
                        "<p>The Suzuki DR650, Kawasaki KLR650, and Honda XR650L are simple reliable off-road-capable motorcycles with aftermarket fuel tanks and luggage racks.</p>",
                    ),
                ),
            ]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["best cruiser motorcycle offroad easy repair"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": "The Suzuki DR650 is the best fit[1].", "topics": [{"topic": "Best overall: Suzuki DR650", "summary": "The Suzuki DR650 is a better fit than a luxury cruiser because the source groups it with simple reliable off-road-capable motorcycles and notes aftermarket fuel tanks and luggage racks[1].", "tags": [{"term": "Suzuki DR650"}]}]}"#,
            r#"{"title_answer": "The Suzuki DR650 is the best fit[1].", "topics": [{"topic": "Best overall: Suzuki DR650", "summary": "The Suzuki DR650 is a better fit than a luxury cruiser because the source groups it with simple reliable off-road-capable motorcycles and notes aftermarket fuel tanks and luggage racks[1].", "tags": [{"term": "Suzuki DR650"}]}]}"#,
        ]);

        let result = WebResearchEngine::new(&llm, &browser)
            .research(question, question, &mut |_| {})
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].url, "https://good.example/dual-sports");
        assert!(result
            .title_answer
            .as_deref()
            .unwrap_or_default()
            .contains("DR650"));
    }

    #[tokio::test]
    async fn motorcycle_research_rejects_bicycle_sources() {
        let question = "what is a good cruiser bike that is easy to fix and reliable and can also go offroad? I need right to repair, MC repair, aux gas tanks, WABDR gravel roads, and luggage.";
        let browser = StubBrowser {
            search_html: search_html_with_results(&[
                (
                    "https://bike.example/cruiser",
                    "What Are the Best Cruiser Bicycles For Riding On Trails?",
                    "Cruiser bicycles, cycling, pedals, e-bike trails, beach cruiser comfort.",
                ),
                (
                    "https://bike.example/gravel",
                    "Best gravel bikes",
                    "Road bike and gravel bike buyer guide for cyclists.",
                ),
            ]),
            pages: HashMap::new(),
        };

        let llm = StubLlm::new([r#"{"queries": ["best cruiser bike off road"]}"#]);

        let err = WebResearchEngine::new(&llm, &browser)
            .research(question, question, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("wrong kind of bike"));
    }

    #[tokio::test]
    async fn constrained_recommendation_audit_replaces_unsupported_bad_fit() {
        let question = "what is a good cruiser bike that is easy to fix and reliable and can also go offroad? My constraints are right to repair, gravel roads, and easy roadside repair.";
        let browser = StubBrowser {
            search_html: search_html_with_results(&[(
                "https://bad.example/gold-wing",
                "Best touring motorcycles",
                "Honda Gold Wing luxury comfort power road touring.",
            )]),
            pages: HashMap::from([(
                "https://bad.example/gold-wing".to_string(),
                page_html(
                    "Gold Wing",
                    "<p>The Honda Gold Wing is a luxury touring motorcycle with road comfort.</p>",
                ),
            )]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["best cruiser motorcycle offroad easy repair"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": "The Honda Gold Wing is the best fit[1].", "topics": [{"topic": "Best overall: Honda Gold Wing", "summary": "The Honda Gold Wing is suitable for gravel and easy roadside repair because it is comfortable and powerful[1].", "tags": [{"term": "Honda Gold Wing"}]}]}"#,
            r#"{"title_answer": "The gathered source does not support recommending the Honda Gold Wing for the user's off-road and repairability constraints[1].", "topics": []}"#,
        ]);

        let result = WebResearchEngine::new(&llm, &browser)
            .research(question, question, &mut |_| {})
            .await
            .unwrap();

        assert!(result
            .title_answer
            .as_deref()
            .unwrap_or_default()
            .contains("does not support"));
        assert!(result.topics.is_empty());
    }

    #[tokio::test]
    async fn research_runs_end_to_end_and_produces_citations() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::from([
                (
                    "https://a.example/article".to_string(),
                    page_html("Source A", "<p>Magnesium may help sleep.</p>"),
                ),
                (
                    "https://b.example/article".to_string(),
                    page_html(
                        "Source B",
                        "<p>Effects on insulin resistance are modest.</p>",
                    ),
                ),
            ]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["magnesium and sleep research"]}"#,
            r#"{"picks": [0, 1]}"#,
            r#"{"title_answer": null, "topics": [{"topic": "Magnesium and sleep", "summary": "Magnesium may improve sleep[1] with modest effects on insulin resistance[2].", "tags": [{"term": "magnesium"}]}]}"#,
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let mut progress_log = Vec::new();
        let result = engine
            .research("Does magnesium help sleep?", "Some seed text.", &mut |m| {
                progress_log.push(m.to_string());
            })
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 2);
        assert_eq!(result.citations[0].url, "https://a.example/article");
        assert_eq!(result.citations[1].url, "https://b.example/article");
        assert_eq!(result.topics.len(), 1);
        assert!(result.topics[0].summary.contains("[1]"));
        assert!(result.topics[0].summary.contains("[2]"));
        assert_eq!(result.topics[0].tags[0].term, "magnesium");
        assert!(!progress_log.is_empty());
    }

    #[tokio::test]
    async fn synthesis_strips_closed_think_blocks_before_json() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::from([(
                "https://a.example/article".to_string(),
                page_html(
                    "Source A",
                    "<p>Odin is used for Samsung firmware flashing.</p>",
                ),
            )]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["Samsung Odin firmware flashing"]}"#,
            r#"{"picks": [0]}"#,
            r#"<think>I should compare the source to the claim.</think>
{"title_answer": null, "topics": [{"topic": "Samsung Odin", "summary": "Odin is used for Samsung firmware flashing[1].", "tags": [{"term": "Odin"}]}]}"#,
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let result = engine
            .research("Samsung Odin update", "Odin install notes", &mut |_| {})
            .await
            .unwrap();

        assert_eq!(result.topics.len(), 1);
        assert_eq!(result.topics[0].topic, "Samsung Odin");
        assert!(result.topics[0].summary.contains("[1]"));
    }

    #[tokio::test]
    async fn synthesis_retries_when_first_response_is_reasoning_only() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::from([(
                "https://a.example/article".to_string(),
                page_html(
                    "Source A",
                    "<p>Odin is an internal Samsung flashing tool.</p>",
                ),
            )]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["Samsung Odin flashing tool"]}"#,
            r#"{"picks": [0]}"#,
            "<think>\nOkay, let's see. I need to analyze the source but never emit JSON.",
            r#"{"title_answer": null, "topics": [{"topic": "Odin flashing tool", "summary": "Odin is described as a Samsung flashing tool[1].", "tags": [{"term": "Odin"}]}]}"#,
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let result = engine
            .research("Samsung Odin update", "Odin install notes", &mut |_| {})
            .await
            .unwrap();

        assert_eq!(result.topics[0].topic, "Odin flashing tool");
    }

    #[tokio::test]
    async fn synthesis_falls_back_to_cited_text_when_json_retry_is_truncated() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::from([(
                "https://a.example/article".to_string(),
                page_html(
                    "Source A",
                    "<p>Simple motorcycles are often easier to maintain on long trips.</p>",
                ),
            )]),
        };

        let llm = StubLlm::new([
            r#"{"queries": ["simple adventure motorcycle reliability"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": null, "topics": [{"topic": "Simple motorcycles", "summary": "Simple motorcycles"#,
            r#"{"title_answer": null, "topics": [{"topic": "Simple motorcycles", "summary": "Simple motorcycles"#,
            "For remote travel, simpler motorcycles are easier to maintain and repair on the road[1].",
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let result = engine
            .research("Reliable travel motorcycle", "right to repair", &mut |_| {})
            .await
            .unwrap();

        assert_eq!(result.topics.len(), 1);
        assert_eq!(result.topics[0].topic, "Web research");
        assert!(result.topics[0].summary.contains("simpler motorcycles"));
        assert!(result.topics[0].summary.contains("[1]"));
    }

    #[tokio::test]
    async fn search_continues_when_one_planned_query_fails() {
        struct FailingFirstQueryBrowser {
            search_html: String,
            page_html: String,
        }

        impl BrowserDriver for FailingFirstQueryBrowser {
            fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
                Box::pin(async move {
                    if url.contains("bad") {
                        return Err(CoreError::Other("HTTP 429 Too Many Requests".to_string()));
                    }
                    let bytes = if url.starts_with("https://search.brave.com/")
                        || url.starts_with("https://html.duckduckgo.com/")
                    {
                        self.search_html.clone().into_bytes()
                    } else if url == "https://a.example/article" {
                        self.page_html.clone().into_bytes()
                    } else {
                        return Err(CoreError::Other(format!("no stub page for {url}")));
                    };
                    Ok(FetchedResource {
                        url: url.to_string(),
                        content_type: Some("text/html".to_string()),
                        bytes,
                    })
                })
            }
        }

        let browser = FailingFirstQueryBrowser {
            search_html: search_html_with_two_results(),
            page_html: page_html("Source A", "<p>Good query found a usable source.</p>"),
        };
        let llm = StubLlm::new([
            r#"{"queries": ["bad query", "good query"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": "The good query still produced an answer[1].", "topics": []}"#,
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let result = engine
            .research("Question needing web", "seed", &mut |_| {})
            .await
            .unwrap();

        assert_eq!(
            result.title_answer.as_deref(),
            Some("The good query still produced an answer[1].")
        );
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].url, "https://a.example/article");
    }

    #[test]
    fn reasoning_only_structured_response_error_hides_thinking_text() {
        let err = clean_structured_response(
            "<think>\nOkay, let's see. This hidden reasoning should not leak.",
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("reasoning-only response hidden"));
        assert!(!err.contains("Okay, let's see"));
        assert!(!err.contains("hidden reasoning"));
    }

    #[tokio::test]
    async fn skips_unreachable_sources_instead_of_failing_the_whole_run() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::from([(
                "https://a.example/article".to_string(),
                page_html("Source A", "<p>Content.</p>"),
            )]),
        };
        // Source B has no stub page registered, so fetching it will error
        // and should be silently skipped rather than failing the run.

        let llm = StubLlm::new([
            r#"{"queries": ["some query"]}"#,
            r#"{"picks": [0, 1]}"#,
            r#"{"title_answer": null, "topics": [{"topic": "T", "summary": "S[1].", "tags": []}]}"#,
        ]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let result = engine
            .research("Some title", "seed", &mut |_| {})
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].url, "https://a.example/article");
    }

    #[tokio::test]
    async fn no_search_results_returns_a_clear_error() {
        let browser = StubBrowser {
            search_html: "<html><body>no results</body></html>".to_string(),
            pages: HashMap::new(),
        };
        let llm = StubLlm::new([r#"{"queries": ["a query"]}"#]);

        let engine = WebResearchEngine::new(&llm, &browser);
        let err = engine
            .research("Title", "seed", &mut |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No search results"));
    }

    /// A `StubBrowser` variant that trips a cancellation flag the moment it
    /// serves the search-results page, and records every URL it is asked to
    /// fetch — so a test can prove a run stops *after* searching but *before*
    /// reading any source.
    struct CancelOnSearchBrowser {
        search_html: String,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        fetched: Mutex<Vec<String>>,
    }

    impl BrowserDriver for CancelOnSearchBrowser {
        fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
            Box::pin(async move {
                self.fetched.lock().unwrap().push(url.to_string());
                if url.starts_with("https://search.brave.com/") {
                    // Simulate the user hitting Stop while results come back.
                    self.cancel
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    Ok(FetchedResource {
                        url: url.to_string(),
                        content_type: Some("text/html".to_string()),
                        bytes: self.search_html.clone().into_bytes(),
                    })
                } else {
                    Err(CoreError::Other(format!(
                        "should not fetch {url} after cancel"
                    )))
                }
            })
        }
    }

    #[tokio::test]
    async fn cancellation_before_first_search_returns_cancelled() {
        let browser = StubBrowser {
            search_html: search_html_with_two_results(),
            pages: HashMap::new(),
        };
        let llm = StubLlm::new([r#"{"queries": ["q"]}"#]);
        let engine = WebResearchEngine::new(&llm, &browser);

        // Flag already tripped before the run begins → stop at the first
        // between-search checkpoint, before any network fetch.
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let err = engine
            .research_cancellable("Title", "seed", Some(&cancel), &mut |_| {})
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), RESEARCH_CANCELLED);
    }

    #[tokio::test]
    async fn cancellation_between_search_and_reading_stops_before_any_fetch() {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let browser = CancelOnSearchBrowser {
            search_html: search_html_with_two_results(),
            cancel: cancel.clone(),
            fetched: Mutex::new(Vec::new()),
        };
        // The LLM plans a query and picks sources, but the run must stop before
        // synthesize is ever reached — no synth response is queued, so reaching
        // it would surface a *different* error than RESEARCH_CANCELLED.
        let llm = StubLlm::new([r#"{"queries": ["q"]}"#, r#"{"picks": [0, 1]}"#]);
        let engine = WebResearchEngine::new(&llm, &browser);

        let err = engine
            .research_cancellable("Title", "seed", Some(&cancel), &mut |_| {})
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), RESEARCH_CANCELLED);

        // Only the search page was fetched; no source article was read.
        let fetched = browser.fetched.lock().unwrap();
        assert_eq!(fetched.len(), 1, "should stop before reading any source");
        assert!(fetched[0].starts_with("https://search.brave.com/"));
    }
}

/// The queue-backed stub `LlmProvider` is reused by the deep-research
/// [`crate::research::agent`] tests, which need the same "reply with the next
/// canned response" behaviour to drive their multi-round loop deterministically.
#[cfg(test)]
pub(crate) use tests::StubLlm;
