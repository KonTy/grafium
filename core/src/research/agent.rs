//! The Deep Research agent: the multi-round loop that turns a question into a
//! cited answer, and the reason this feature exists at all.
//!
//! [`crate::ai::web_research::WebResearchEngine`] does one pass — plan, search,
//! read, synthesize — and treats every stumble as fatal: an empty result set, a
//! rate-limited engine, or a first guess at queries that simply missed the good
//! sources all end the run with "sorry, nothing found". A student researching an
//! unfamiliar topic hits exactly those conditions, so this engine instead
//! behaves the way a person actually researches:
//!
//! 1. **plan** search queries from the question,
//! 2. **search** every enabled engine (via [`crate::scraping::engines::search_all`],
//!    which already tolerates any single engine failing),
//! 3. **select** which results are worth reading,
//! 4. **fetch + extract** them (PDFs included; optional OCR for scanned ones),
//! 5. **assess** whether what's been gathered can actually answer the question,
//! 6. **refine** the queries toward whatever is still missing and loop, up to
//!    `max_rounds`,
//! 7. **synthesize** a cited answer once it has enough (or the rounds run out).
//!
//! ## The one rule that drives the whole design: never give up after one failure
//!
//! Every failure mode the old engine died on is, here, a *survivable* event that
//! the next round is meant to recover from. A round that finds zero candidates
//! does not end the run — it refines and searches again. A source that won't
//! fetch or won't extract is recorded as unreadable and skipped, not fatal. Only
//! two things end the loop: the model judging it has *enough* (assess returns
//! sufficient), or exhausting `max_rounds`. Even then, as long as *any* source
//! was read, the run synthesizes an answer from it rather than erroring; the
//! sole hard error is finding genuinely nothing across every round (e.g. this
//! machine's IP throttled by every web engine at once), which is a real
//! "couldn't research this" the user needs told.
//!
//! ## Output shape is deliberately identical to single-round web research
//!
//! The loop returns a [`WebResearchResult`] — the exact type
//! [`crate::ai::web_research`] produces — so the knowledge layer renders a Deep
//! Research answer with the same "From the web" section, `[n]` citation list,
//! and tag handling it already uses, with no second rendering path to maintain.
//!
//! ## Robustness of the LLM steps
//!
//! Each step's prompt is user-editable ([`crate::research::config::ResearchPrompts`])
//! and used as the *system* prompt, with the concrete data appended as a user
//! message, so a student can rewrite how a step reasons without breaking a
//! placeholder. Because the loop parses the model's structured output, two
//! defenses are non-negotiable: reasoning models' `<think>` blocks are stripped
//! before parsing (otherwise chain-of-thought gets parsed as JSON), and the
//! machine-readable steps (select, assess) are parsed *tolerantly* — a garbled
//! answer falls back to a safe default (read nothing extra / treat as
//! sufficient) instead of aborting a run the user is waiting on.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Deserialize;

use crate::ai::reasoning::{strip_think_blocks, ThinkStripResult};
use crate::ai::references::{clean_tag_terms, concept_parse_error, extract_json_object, TagJson};
use crate::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::ai::web_research::{
    cancelled_error, is_cancelled, truncate, Citation, ResearchTopic, WebResearchResult,
};
use crate::error::{CoreError, Result};
use crate::research::config::ResearchConfig;
use crate::scraping::browser::BrowserDriver;
use crate::scraping::search::SearchResult;
use crate::scraping::{engines, extract};

// ── Per-step text budgets ────────────────────────────────────────────────────
//
// A local model's context window is the scarce resource here, and the loop can
// accumulate many sources across rounds, so each step gets only as much text as
// it needs for *its* decision:
// - selection ranks on the snippet alone, so candidate text is heavily clipped;
// - assessment only judges whether coverage exists, so a modest excerpt per
//   source keeps the (multi-source) assess prompt affordable;
// - synthesis is the one step that must actually quote and cite, so it gets the
//   largest per-source budget.

/// Candidate title length shown to the selection step.
const SELECT_TITLE_CHARS: usize = 100;
/// Candidate snippet length shown to the selection step.
const SELECT_SNIPPET_CHARS: usize = 200;
/// Per-source excerpt length shown to the assessment step. Smaller than the
/// synthesis budget because assessment needs breadth (is the angle covered?),
/// not depth, and every gathered source is included in one prompt.
const ASSESS_EXCERPT_CHARS: usize = 1200;
/// Per-source excerpt length shown to the synthesis step — the biggest budget,
/// since this is where claims are actually written and cited.
const SYNTH_EXCERPT_CHARS: usize = 2800;
/// Question length passed to the query-planning/refining steps; questions are
/// normally short, this just guards against a pathologically long paste.
const QUESTION_CHARS: usize = 2000;
/// Upper bound on candidates handed to the selection step in a round. With
/// several enabled engines times several queries, the raw candidate list can be
/// large; ranking is order-preserving (web engines first, then academic), so
/// clipping keeps a representative spread without an unbounded select prompt.
const MAX_CANDIDATES_TO_RANK: usize = 40;
/// Pages rendered when OCR-ing a scanned PDF (see [`crate::research::ocr`]).
const OCR_MAX_PAGES: usize = 5;

/// Which step of the loop is running, for live progress. The string forms are
/// the phase identifiers the UI switches on, and match the frozen research
/// contract (`searching_web`/`reading_sources` are shared with the existing
/// notes+web ask flow so the same status chips light up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchPhase {
    Planning,
    Searching,
    Reading,
    Assessing,
    Refining,
    Synthesizing,
}

impl ResearchPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ResearchPhase::Planning => "planning",
            ResearchPhase::Searching => "searching_web",
            ResearchPhase::Reading => "reading_sources",
            ResearchPhase::Assessing => "assessing",
            ResearchPhase::Refining => "refining",
            ResearchPhase::Synthesizing => "synthesizing",
        }
    }
}

/// A progress event from a running research loop: either a coarse phase change
/// (which drives the UI's status label) or a fine-grained human-readable note
/// (which source is being read, that a round found nothing and is retrying).
/// Mirrors the [`crate::knowledge`] ask flow's phase/note split so the two map
/// onto the same stream events.
pub enum ResearchProgress<'a> {
    Phase(ResearchPhase),
    Note(&'a str),
}

/// Runs one Deep Research loop over a borrowed [`ResearchConfig`] (its engine
/// list, editable prompts, and round/source/OCR knobs).
pub struct DeepResearchEngine<'a> {
    llm: &'a dyn LlmProvider,
    browser: &'a dyn BrowserDriver,
    config: &'a ResearchConfig,
}

impl<'a> DeepResearchEngine<'a> {
    pub fn new(
        llm: &'a dyn LlmProvider,
        browser: &'a dyn BrowserDriver,
        config: &'a ResearchConfig,
    ) -> Self {
        Self {
            llm,
            browser,
            config,
        }
    }

    /// Uncancellable convenience form — see [`Self::research_cancellable`].
    pub async fn research(
        &self,
        question: &str,
        progress: &mut (dyn FnMut(ResearchProgress) + Send),
    ) -> Result<WebResearchResult> {
        self.research_cancellable(question, None, progress).await
    }

    /// Run the full plan→search→select→read→assess→refine→synthesize loop for
    /// `question`, reporting each step through `progress`.
    ///
    /// `cancel` (if given) is polled before every network operation and every
    /// LLM call — the two places a run spends real time — so a Stop actually
    /// interrupts within one in-flight request rather than waiting out the whole
    /// remaining loop. On cancellation the run returns [`cancelled_error`] so
    /// the caller can stay silent instead of surfacing a scary failure. It is
    /// also threaded into each [`CompletionOptions::cancel`] so a slow *local*
    /// generation can be aborted mid-token, not just between steps.
    ///
    /// Returns an error only when the loop finishes without a single readable
    /// source (genuinely nothing found) or the synthesis step itself fails;
    /// every lesser failure is absorbed and retried in a later round.
    pub async fn research_cancellable(
        &self,
        question: &str,
        cancel: Option<Arc<AtomicBool>>,
        progress: &mut (dyn FnMut(ResearchProgress) + Send),
    ) -> Result<WebResearchResult> {
        let cancel_ref = cancel.as_deref();

        // ── Plan (round 1 queries) ──────────────────────────────────────────
        if is_cancelled(cancel_ref) {
            return Err(cancelled_error());
        }
        progress(ResearchProgress::Phase(ResearchPhase::Planning));
        // A failed/garbled plan step must not sink the run: fall back to
        // searching the literal question, which is a perfectly reasonable first
        // query and keeps the "never give up" promise even here.
        let mut queries = self
            .plan_queries(question, cancel.clone())
            .await
            .unwrap_or_default();
        if queries.is_empty() {
            queries = vec![question.trim().to_string()];
        }

        // State accumulated across rounds.
        let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut tried_queries: Vec<String> = Vec::new();
        let mut citations: Vec<Citation> = Vec::new();
        let mut excerpts: Vec<(usize, String)> = Vec::new();
        let mut titles: Vec<String> = Vec::new();

        let max_rounds = self.config.max_rounds.max(1);
        let mut round = 0;
        loop {
            round += 1;
            let last_round = round >= max_rounds;

            // ── Search ──────────────────────────────────────────────────────
            if is_cancelled(cancel_ref) {
                return Err(cancelled_error());
            }
            progress(ResearchProgress::Phase(ResearchPhase::Searching));
            let note = format!(
                "Round {round}: searching {} quer{}",
                queries.len(),
                plural(queries.len())
            );
            progress(ResearchProgress::Note(&note));

            let mut candidates: Vec<SearchResult> = Vec::new();
            let mut candidate_urls: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for query in &queries {
                if is_cancelled(cancel_ref) {
                    return Err(cancelled_error());
                }
                let results = engines::search_all(
                    self.browser,
                    self.config,
                    query,
                    self.config.results_per_query,
                    cancel_ref,
                )
                .await;
                for result in results {
                    // Dedup within the round and against everything already
                    // fetched in previous rounds — never re-rank a source we've
                    // read, never list the same URL twice.
                    if seen_urls.contains(&result.url) {
                        continue;
                    }
                    if candidate_urls.insert(result.url.clone()) {
                        candidates.push(result);
                    }
                }
                tried_queries.push(query.clone());
            }
            candidates.truncate(MAX_CANDIDATES_TO_RANK);

            // ── Select + read ───────────────────────────────────────────────
            if !candidates.is_empty() {
                if is_cancelled(cancel_ref) {
                    return Err(cancelled_error());
                }
                // Tolerant: a broken selection response shouldn't waste the
                // round — fall back to reading the top candidates in rank order.
                let picked = match self
                    .select_sources(question, &candidates, cancel.clone())
                    .await
                {
                    Ok(picked) if !picked.is_empty() => picked,
                    _ => candidates.clone(),
                };

                progress(ResearchProgress::Phase(ResearchPhase::Reading));
                let remaining = self.config.max_sources.saturating_sub(citations.len());
                for (index, candidate) in picked.iter().take(remaining).enumerate() {
                    if is_cancelled(cancel_ref) {
                        return Err(cancelled_error());
                    }
                    // Mark as attempted *before* fetching so a source that fails
                    // is never retried in a later round (dedup covers failures,
                    // not just successes).
                    if !seen_urls.insert(candidate.url.clone()) {
                        continue;
                    }
                    let note = format!(
                        "Reading source {}/{}: {}",
                        index + 1,
                        picked.len().min(remaining),
                        truncate(&candidate.title, 80)
                    );
                    progress(ResearchProgress::Note(&note));

                    match self.read_source(candidate).await {
                        Some(text) => {
                            let number = citations.len() + 1;
                            citations.push(Citation {
                                number,
                                title: candidate.title.clone(),
                                url: candidate.url.clone(),
                            });
                            titles.push(candidate.title.clone());
                            excerpts.push((number, text));
                        }
                        None => {
                            let note = format!(
                                "Couldn't read {}; skipping",
                                truncate(&candidate.title, 80)
                            );
                            progress(ResearchProgress::Note(&note));
                        }
                    }
                }
            }

            // ── Decide whether to stop ──────────────────────────────────────
            let budget_reached = citations.len() >= self.config.max_sources;

            if last_round || budget_reached {
                break;
            }

            if citations.is_empty() {
                // Nothing readable yet, but rounds remain: this is the exact
                // "one failed search must not end the run" case. Refine toward
                // the question from a different angle and try again.
                progress(ResearchProgress::Phase(ResearchPhase::Refining));
                progress(ResearchProgress::Note(
                    "No usable sources yet — refining the search and trying again",
                ));
                queries = self
                    .refine_queries(
                        question,
                        &titles,
                        "No usable sources were found yet; try different, more specific queries.",
                        &tried_queries,
                        cancel.clone(),
                    )
                    .await
                    .unwrap_or_default();
                if queries.is_empty() {
                    queries = vec![question.trim().to_string()];
                }
                continue;
            }

            // Have material and rounds left: ask whether it's enough.
            if is_cancelled(cancel_ref) {
                return Err(cancelled_error());
            }
            progress(ResearchProgress::Phase(ResearchPhase::Assessing));
            // Tolerant: if the verdict can't be parsed, treat it as sufficient
            // and synthesize what we have rather than burning more rounds on an
            // anomaly — the user gets an answer instead of extra latency.
            let (sufficient, missing) = self
                .assess_sufficiency(question, &excerpts, cancel.clone())
                .await
                .unwrap_or((true, String::new()));
            if sufficient {
                break;
            }

            progress(ResearchProgress::Phase(ResearchPhase::Refining));
            let note = if missing.trim().is_empty() {
                "Not enough yet — refining the search".to_string()
            } else {
                format!("Still missing: {}", truncate(&missing, 160))
            };
            progress(ResearchProgress::Note(&note));
            queries = self
                .refine_queries(question, &titles, &missing, &tried_queries, cancel.clone())
                .await
                .unwrap_or_default();
            if queries.is_empty() {
                // Even a failed refine must not stall the loop.
                queries = vec![question.trim().to_string()];
            }
        }

        // ── Synthesize ──────────────────────────────────────────────────────
        if citations.is_empty() {
            // Survived every round but truly found nothing readable — a real
            // failure the user needs told (commonly: every web engine throttled
            // this IP at once, and the academic APIs had no hits).
            return Err(CoreError::Other(
                "Deep Research couldn't find or read any usable sources for this question, \
                 even after refining. The search engines may be rate-limited right now, or the \
                 question may need rephrasing."
                    .to_string(),
            ));
        }

        if is_cancelled(cancel_ref) {
            return Err(cancelled_error());
        }
        progress(ResearchProgress::Phase(ResearchPhase::Synthesizing));
        progress(ResearchProgress::Note("Writing the cited answer"));
        let (title_answer, topics) = self.synthesize(question, &excerpts, cancel).await?;

        Ok(WebResearchResult {
            title_answer,
            topics,
            citations,
        })
    }

    /// Fetch and extract one source's readable text, returning `None` (never an
    /// error) when it can't be read — an unreachable page, an un-extractable
    /// document, or an empty/scanned PDF that OCR couldn't (or wasn't allowed
    /// to) recover. The loop treats every `None` the same: record it as skipped
    /// and move on, so no single bad source can end the run.
    async fn read_source(&self, candidate: &SearchResult) -> Option<String> {
        let resource = self.browser.fetch(&candidate.url).await.ok()?;
        let text = match extract::extract(&resource) {
            Ok(content) if !content.text.trim().is_empty() => content.text,
            // Extraction failed, or produced no usable text. If it's a PDF and
            // OCR is enabled, try to recover it from the pixels; otherwise this
            // source is simply unreadable.
            _ => {
                if self.config.ocr_enabled && extract::is_pdf(&resource) {
                    // OCR degrades gracefully: `Ok(None)` when tesseract isn't
                    // installed, `Err` only on an unexpected tooling failure —
                    // both mean "unreadable", handled by the `?`/`ok()` below.
                    crate::research::ocr::ocr_pdf(&resource.bytes, OCR_MAX_PAGES)
                        .ok()
                        .flatten()?
                } else {
                    return None;
                }
            }
        };
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    async fn plan_queries(
        &self,
        question: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<String>> {
        let user = format!("Question: {}", truncate(question, QUESTION_CHARS));
        let raw = self
            .complete(&self.config.prompts.plan_queries, &user, 200, 0.2, cancel)
            .await?;
        parse_queries(&raw)
    }

    async fn select_sources(
        &self,
        question: &str,
        candidates: &[SearchResult],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<SearchResult>> {
        #[derive(Deserialize)]
        struct PicksJson {
            #[serde(default)]
            picks: Vec<usize>,
        }

        let mut user = format!(
            "Question: {}\n\nCandidate results (index: title — url — snippet):\n",
            truncate(question, QUESTION_CHARS)
        );
        for (index, candidate) in candidates.iter().enumerate() {
            user.push_str(&format!(
                "{index}: {} — {} — {}\n",
                truncate(&candidate.title, SELECT_TITLE_CHARS),
                candidate.url,
                truncate(&candidate.snippet, SELECT_SNIPPET_CHARS),
            ));
        }

        let raw = self
            .complete(&self.config.prompts.select_sources, &user, 150, 0.0, cancel)
            .await?;
        let cleaned = strip_reasoning(&raw);
        let json = extract_json_object(cleaned.trim())?;
        let parsed: PicksJson = serde_json::from_str(json)
            .map_err(|e| concept_parse_error(&format!("invalid source-pick JSON: {e}"), &raw))?;

        let mut seen = std::collections::HashSet::new();
        Ok(parsed
            .picks
            .into_iter()
            .filter(|index| seen.insert(*index))
            .filter_map(|index| candidates.get(index).cloned())
            .collect())
    }

    async fn assess_sufficiency(
        &self,
        question: &str,
        excerpts: &[(usize, String)],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(bool, String)> {
        #[derive(Deserialize)]
        struct AssessJson {
            #[serde(default)]
            sufficient: bool,
            #[serde(default)]
            missing: String,
        }

        let mut user = format!(
            "Question: {}\n\nExcerpts from the sources gathered so far:\n",
            truncate(question, QUESTION_CHARS)
        );
        for (number, text) in excerpts {
            user.push_str(&format!(
                "[{number}]:\n{}\n\n",
                truncate(text, ASSESS_EXCERPT_CHARS)
            ));
        }

        let raw = self
            .complete(
                &self.config.prompts.assess_sufficiency,
                &user,
                200,
                0.0,
                cancel,
            )
            .await?;
        let cleaned = strip_reasoning(&raw);
        let json = extract_json_object(cleaned.trim())?;
        let parsed: AssessJson = serde_json::from_str(json)
            .map_err(|e| concept_parse_error(&format!("invalid assessment JSON: {e}"), &raw))?;
        Ok((parsed.sufficient, parsed.missing.trim().to_string()))
    }

    async fn refine_queries(
        &self,
        question: &str,
        titles: &[String],
        missing: &str,
        tried: &[String],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<String>> {
        let mut user = format!("Question: {}\n\n", truncate(question, QUESTION_CHARS));
        if titles.is_empty() {
            user.push_str("Titles gathered so far: (none)\n\n");
        } else {
            user.push_str("Titles gathered so far:\n");
            for title in titles {
                user.push_str(&format!("- {}\n", truncate(title, 120)));
            }
            user.push('\n');
        }
        user.push_str(&format!("Still missing: {}\n\n", truncate(missing, 400)));
        if !tried.is_empty() {
            user.push_str("Earlier queries (do NOT repeat these):\n");
            for query in tried {
                user.push_str(&format!("- {query}\n"));
            }
        }

        let raw = self
            .complete(&self.config.prompts.refine_queries, &user, 200, 0.2, cancel)
            .await?;
        // Drop any refined query that merely repeats one we already ran — a
        // repeat would waste the round it was meant to rescue.
        let already: std::collections::HashSet<String> =
            tried.iter().map(|q| q.to_lowercase()).collect();
        Ok(parse_queries(&raw)?
            .into_iter()
            .filter(|q| !already.contains(&q.to_lowercase()))
            .collect())
    }

    async fn synthesize(
        &self,
        question: &str,
        excerpts: &[(usize, String)],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(Option<String>, Vec<ResearchTopic>)> {
        #[derive(Deserialize)]
        struct TopicJson {
            #[serde(default)]
            topic: String,
            #[serde(default)]
            summary: String,
            #[serde(default)]
            tags: Vec<TagJson>,
        }

        #[derive(Deserialize)]
        struct SynthesisJson {
            #[serde(default)]
            title_answer: Option<String>,
            #[serde(default)]
            topics: Vec<TopicJson>,
        }

        let mut user = format!("Question: {}\n\n", truncate(question, QUESTION_CHARS));
        user.push_str("Sources (numbered — cite these numbers as [n] in your answer):\n");
        for (number, text) in excerpts {
            user.push_str(&format!(
                "[{number}]:\n{}\n\n",
                truncate(text, SYNTH_EXCERPT_CHARS)
            ));
        }

        let raw = self
            .complete(&self.config.prompts.synthesize, &user, 1400, 0.3, cancel)
            .await?;
        let cleaned = strip_reasoning(&raw);
        let trimmed = cleaned.trim();
        let json = extract_json_object(trimmed)?;
        let parsed: SynthesisJson = serde_json::from_str(json).map_err(|e| {
            concept_parse_error(&format!("invalid research synthesis JSON: {e}"), trimmed)
        })?;

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

    /// Shared LLM call: the user-editable prompt for the step is the *system*
    /// prompt, the step's concrete data is the user message, and the run's
    /// cancel flag is threaded through so a slow local generation can be
    /// aborted mid-token.
    async fn complete(
        &self,
        system_prompt: &str,
        user: &str,
        max_tokens: u32,
        temperature: f32,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        if is_cancelled(cancel.as_deref()) {
            return Err(cancelled_error());
        }
        let messages = [ChatMessage {
            role: MessageRole::User,
            content: user.to_string(),
        }];
        let options = CompletionOptions {
            max_tokens: Some(max_tokens),
            temperature: Some(temperature),
            system_prompt: Some(system_prompt.to_string()),
            stop: None,
            cancel,
        };
        self.llm.complete(&messages, &options).await
    }
}

/// Parse a `{"queries": [...]}` object into a cleaned, de-duplicated,
/// non-empty query list. Shared by planning and refining, and tolerant of the
/// `<think>` blocks a reasoning model emits before its JSON.
fn parse_queries(raw: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct QueriesJson {
        #[serde(default)]
        queries: Vec<String>,
    }

    let cleaned = strip_reasoning(raw);
    let json = extract_json_object(cleaned.trim())?;
    let parsed: QueriesJson = serde_json::from_str(json)
        .map_err(|e| concept_parse_error(&format!("invalid search-query JSON: {e}"), raw))?;

    let mut seen = std::collections::HashSet::new();
    Ok(parsed
        .queries
        .into_iter()
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .filter(|q| seen.insert(q.to_lowercase()))
        .collect())
}

/// Strip `<think>…</think>` from a completed response before parsing it as
/// JSON. A reasoning model that produced *only* reasoning yields an empty
/// string, which then fails JSON extraction with a clear error rather than the
/// loop mistaking chain-of-thought for structured output.
fn strip_reasoning(raw: &str) -> String {
    match strip_think_blocks(raw) {
        ThinkStripResult::Answer(text) => text,
        ThinkStripResult::ReasoningOnly => String::new(),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::web_research::StubLlm;
    use crate::research::config::{
        EngineCategory, EngineKind, JsonPaths, ResearchConfig, SearchEngineDef,
    };
    use crate::scraping::browser::{FetchedResource, MockBrowserDriver};
    use crate::scraping::engines::build_url;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    const SEARCH_TEMPLATE: &str = "https://s.test/api?q={query}";

    fn test_engine() -> SearchEngineDef {
        SearchEngineDef {
            id: "test".to_string(),
            name: "Test".to_string(),
            kind: EngineKind::Json,
            url_template: SEARCH_TEMPLATE.to_string(),
            enabled: true,
            builtin: false,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results".to_string(),
                url: "url".to_string(),
                title: "title".to_string(),
                snippet: "snippet".to_string(),
            }),
        }
    }

    fn config_with(engines: Vec<SearchEngineDef>, max_rounds: usize) -> ResearchConfig {
        ResearchConfig {
            engines,
            max_rounds,
            results_per_query: 6,
            max_sources: 8,
            ..Default::default()
        }
    }

    /// A JSON search-results resource for the given `(url, title, snippet)`s.
    fn search_results(items: &[(&str, &str, &str)]) -> FetchedResource {
        let body: Vec<String> = items
            .iter()
            .map(|(url, title, snippet)| {
                format!(r#"{{"url":"{url}","title":"{title}","snippet":"{snippet}"}}"#)
            })
            .collect();
        FetchedResource {
            url: String::new(),
            content_type: Some("application/json".to_string()),
            bytes: format!("{{\"results\":[{}]}}", body.join(",")).into_bytes(),
        }
    }

    /// An HTML article page with real extractable body text.
    fn article(url: &str, title: &str, body: &str) -> FetchedResource {
        FetchedResource {
            url: url.to_string(),
            content_type: Some("text/html".to_string()),
            bytes: format!(
                "<html><head><title>{title}</title></head><body><p>{body}</p></body></html>"
            )
            .into_bytes(),
        }
    }

    fn query_url(query: &str) -> String {
        build_url(SEARCH_TEMPLATE, query)
    }

    #[tokio::test]
    async fn multi_round_refinement_actually_happens() {
        // Round 1's query finds one source; the model judges it insufficient and
        // refines to a *new* query that surfaces a second source only reachable
        // that way. Proof of iteration: the second citation cannot appear
        // without the refine→search→read of round 2.
        let mut pages = HashMap::new();
        pages.insert(
            query_url("q1"),
            search_results(&[("https://doc1.test", "Doc One", "s1")]),
        );
        pages.insert(
            query_url("q2"),
            search_results(&[("https://doc2.test", "Doc Two", "s2")]),
        );
        pages.insert(
            "https://doc1.test".to_string(),
            article(
                "https://doc1.test",
                "Doc One",
                "First finding about the topic.",
            ),
        );
        pages.insert(
            "https://doc2.test".to_string(),
            article(
                "https://doc2.test",
                "Doc Two",
                "Second finding filling the gap.",
            ),
        );
        let browser = MockBrowserDriver { pages };
        let config = config_with(vec![test_engine()], 2);

        let llm = StubLlm::new([
            r#"{"queries": ["q1"]}"#,                                       // plan
            r#"{"picks": [0]}"#,                                            // select (round 1)
            r#"{"sufficient": false, "missing": "need the second angle"}"#, // assess
            r#"{"queries": ["q2"]}"#,                                       // refine
            r#"{"picks": [0]}"#,                                            // select (round 2)
            r#"{"title_answer": null, "topics": [{"topic": "T", "summary": "A[1]. B[2].", "tags": [{"term":"topic"}]}]}"#, // synth
        ]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let mut phases = Vec::new();
        let result = engine
            .research("What about the topic?", &mut |p| {
                if let ResearchProgress::Phase(phase) = p {
                    phases.push(phase.as_str());
                }
            })
            .await
            .unwrap();

        assert_eq!(
            result.citations.len(),
            2,
            "both rounds contributed a source"
        );
        assert_eq!(result.citations[0].url, "https://doc1.test");
        assert_eq!(result.citations[1].url, "https://doc2.test");
        assert!(phases.contains(&"assessing"), "assessed sufficiency");
        assert!(phases.contains(&"refining"), "refined and looped");
        assert!(result.topics[0].summary.contains("[2]"));
    }

    #[tokio::test]
    async fn a_dead_engine_does_not_kill_the_run() {
        // Two enabled engines: one whose search URL is never served (fetch
        // errors — the "dead" engine), one that works. The run must still
        // produce a cited answer.
        let dead = SearchEngineDef {
            id: "dead".to_string(),
            url_template: "https://dead.test/s?q={query}".to_string(),
            ..test_engine()
        };
        let live = test_engine();

        let mut pages = HashMap::new();
        // Only the live engine's query URL is served; dead.test is absent.
        pages.insert(
            query_url("q1"),
            search_results(&[("https://doc.test", "Doc", "s")]),
        );
        pages.insert(
            "https://doc.test".to_string(),
            article("https://doc.test", "Doc", "The finding."),
        );
        let browser = MockBrowserDriver { pages };
        let config = config_with(vec![dead, live], 1);

        let llm = StubLlm::new([
            r#"{"queries": ["q1"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": "Answer[1].", "topics": [{"topic": "T", "summary": "S[1].", "tags": []}]}"#,
        ]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let result = engine.research("q?", &mut |_| {}).await.unwrap();
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].url, "https://doc.test");
    }

    #[tokio::test]
    async fn zero_results_survive_into_another_round() {
        // Round 1 finds nothing at all; with rounds remaining the loop must
        // refine and search again rather than error. No select/assess happens
        // in round 1 (no candidates, no citations), so the LLM queue skips
        // straight from plan to refine.
        let mut pages = HashMap::new();
        pages.insert(query_url("q1"), search_results(&[])); // empty results
        pages.insert(
            query_url("q2"),
            search_results(&[("https://doc.test", "Doc", "s")]),
        );
        pages.insert(
            "https://doc.test".to_string(),
            article("https://doc.test", "Doc", "Recovered on the second try."),
        );
        let browser = MockBrowserDriver { pages };
        let config = config_with(vec![test_engine()], 2);

        let llm = StubLlm::new([
            r#"{"queries": ["q1"]}"#, // plan
            r#"{"queries": ["q2"]}"#, // refine (round 1 found nothing)
            r#"{"picks": [0]}"#,      // select (round 2)
            r#"{"title_answer": null, "topics": [{"topic": "T", "summary": "S[1].", "tags": []}]}"#, // synth
        ]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let result = engine.research("q?", &mut |_| {}).await.unwrap();
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].url, "https://doc.test");
    }

    #[tokio::test]
    async fn dedup_across_rounds_never_refetches_a_source() {
        // Round 2's search re-surfaces round 1's URL alongside a new one. The
        // repeat must be dropped (already fetched), and the new one read — so
        // the source read in round 1 is never fetched a second time.
        let mut pages = HashMap::new();
        pages.insert(
            query_url("q1"),
            search_results(&[("https://doc1.test", "Doc One", "s1")]),
        );
        pages.insert(
            query_url("q2"),
            search_results(&[
                ("https://doc1.test", "Doc One", "s1"), // repeat of round 1
                ("https://doc2.test", "Doc Two", "s2"),
            ]),
        );
        pages.insert(
            "https://doc1.test".to_string(),
            article("https://doc1.test", "Doc One", "First."),
        );
        pages.insert(
            "https://doc2.test".to_string(),
            article("https://doc2.test", "Doc Two", "Second."),
        );
        let browser = CountingBrowser::new(MockBrowserDriver { pages });
        let config = config_with(vec![test_engine()], 2);

        let llm = StubLlm::new([
            r#"{"queries": ["q1"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"sufficient": false, "missing": "more"}"#,
            r#"{"queries": ["q2"]}"#,
            r#"{"picks": [0, 1]}"#,
            r#"{"title_answer": null, "topics": [{"topic": "T", "summary": "A[1]. B[2].", "tags": []}]}"#,
        ]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let result = engine.research("q?", &mut |_| {}).await.unwrap();

        assert_eq!(result.citations.len(), 2);
        assert_eq!(
            *browser.count("https://doc1.test"),
            1,
            "doc1 fetched exactly once"
        );
        assert_eq!(*browser.count("https://doc2.test"), 1);
    }

    #[tokio::test]
    async fn cancellation_mid_round_stops_before_synthesis() {
        // A browser that trips the cancel flag the moment it serves the search
        // page. The run must stop right after searching — before reading any
        // source or synthesizing — and report cancellation, not a failure.
        let cancel = Arc::new(AtomicBool::new(false));
        let browser = CancelOnSearchBrowser {
            search_url: query_url("q1"),
            search_body: search_results(&[("https://doc.test", "Doc", "s")]),
            cancel: cancel.clone(),
            fetched: std::sync::Mutex::new(Vec::new()),
        };
        let config = config_with(vec![test_engine()], 2);
        // Only the plan step runs before the cancel is observed.
        let llm = StubLlm::new([r#"{"queries": ["q1"]}"#]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let err = engine
            .research_cancellable("q?", Some(cancel.clone()), &mut |_| {})
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), crate::ai::web_research::RESEARCH_CANCELLED);

        let fetched = browser.fetched.lock().unwrap();
        assert_eq!(fetched.len(), 1, "only the search page was fetched");
        assert_eq!(fetched[0], query_url("q1"));
    }

    #[tokio::test]
    async fn stops_early_when_the_model_judges_sufficiency_met() {
        // With rounds to spare, a "sufficient: true" verdict after round 1 ends
        // the loop immediately — no refine, straight to synthesis.
        let mut pages = HashMap::new();
        pages.insert(
            query_url("q1"),
            search_results(&[("https://doc.test", "Doc", "s")]),
        );
        pages.insert(
            "https://doc.test".to_string(),
            article(
                "https://doc.test",
                "Doc",
                "Everything needed, in one source.",
            ),
        );
        let browser = MockBrowserDriver { pages };
        let config = config_with(vec![test_engine()], 3); // 3 rounds available…

        let llm = StubLlm::new([
            r#"{"queries": ["q1"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"sufficient": true, "missing": ""}"#, // …but we stop after round 1
            r#"{"title_answer": "Yes[1].", "topics": [{"topic": "T", "summary": "S[1].", "tags": []}]}"#,
        ]);

        let engine = DeepResearchEngine::new(&llm, &browser, &config);
        let mut phases = Vec::new();
        let result = engine
            .research("q?", &mut |p| {
                if let ResearchProgress::Phase(phase) = p {
                    phases.push(phase.as_str());
                }
            })
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 1);
        assert!(phases.contains(&"assessing"));
        assert!(
            !phases.contains(&"refining"),
            "sufficiency stops before any refine"
        );
    }

    // ── Test browsers ────────────────────────────────────────────────────────

    /// Wraps a [`MockBrowserDriver`], counting how many times each URL is
    /// fetched — used to prove cross-round dedup never re-fetches a source.
    struct CountingBrowser {
        inner: MockBrowserDriver,
        counts: std::sync::Mutex<HashMap<String, usize>>,
    }

    impl CountingBrowser {
        fn new(inner: MockBrowserDriver) -> Self {
            Self {
                inner,
                counts: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn count(&self, url: &str) -> Box<usize> {
            Box::new(*self.counts.lock().unwrap().get(url).unwrap_or(&0))
        }
    }

    impl BrowserDriver for CountingBrowser {
        fn fetch<'b>(
            &'b self,
            url: &'b str,
        ) -> crate::async_util::BoxFuture<'b, Result<FetchedResource>> {
            *self
                .counts
                .lock()
                .unwrap()
                .entry(url.to_string())
                .or_insert(0) += 1;
            self.inner.fetch(url)
        }
    }

    /// Serves one canned search page and trips a cancel flag when it does, so a
    /// test can prove the loop stops after searching but before reading.
    struct CancelOnSearchBrowser {
        search_url: String,
        search_body: FetchedResource,
        cancel: Arc<AtomicBool>,
        fetched: std::sync::Mutex<Vec<String>>,
    }

    impl BrowserDriver for CancelOnSearchBrowser {
        fn fetch<'b>(
            &'b self,
            url: &'b str,
        ) -> crate::async_util::BoxFuture<'b, Result<FetchedResource>> {
            self.fetched.lock().unwrap().push(url.to_string());
            let matches_search = url == self.search_url;
            let body = self.search_body.clone();
            let cancel = self.cancel.clone();
            Box::pin(async move {
                if matches_search {
                    cancel.store(true, Ordering::Relaxed);
                    Ok(body)
                } else {
                    Err(CoreError::Other(format!(
                        "should not fetch {url} after cancel"
                    )))
                }
            })
        }
    }
}
