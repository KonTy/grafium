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

use crate::ai::references::{clean_tag_terms, concept_parse_error, extract_json_object, TagJson};
use crate::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};
use crate::parser::TagTerm;
use crate::scraping::browser::BrowserDriver;
use crate::scraping::extract;
use crate::scraping::search::{web_search, SearchResult as WebSearchResult};

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
    pub async fn research(
        &self,
        title: &str,
        seed_text: &str,
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
        for query in &queries {
            let results = web_search(self.browser, query, self.config.results_per_query).await?;
            for result in results {
                if seen_urls.insert(result.url.clone()) {
                    candidates.push(result);
                }
            }
        }
        if candidates.is_empty() {
            return Err(CoreError::Other(
                "No search results were found for this topic.".to_string(),
            ));
        }

        progress("Choosing the most relevant sources...");
        let picked = self.pick_sources(title, &candidates).await?;
        if picked.is_empty() {
            return Err(CoreError::Other(
                "The AI didn't judge any search result as relevant enough to read.".to_string(),
            ));
        }

        let mut citations = Vec::new();
        let mut source_excerpts = Vec::new();
        for (i, candidate) in picked.iter().enumerate() {
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

        let prompt = format!(
            "Title: {title}\n\nContent excerpt:\n{}\n\nSuggest up to {} distinct, well-formed web \
             search engine queries that would find good sources to research and fact-check the \
             subject(s) covered here. Prefer specific, targeted queries over broad/generic ones.",
            truncate(seed_text, 2000),
            self.config.max_queries,
        );
        let messages = [ChatMessage {
            role: MessageRole::User,
            content: prompt,
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
        let json_str = extract_json_object(raw.trim())?;
        let parsed: QueriesJson = serde_json::from_str(json_str)
            .map_err(|e| concept_parse_error(&format!("invalid search-query JSON: {e}"), &raw))?;

        Ok(parsed
            .queries
            .into_iter()
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .take(self.config.max_queries)
            .collect())
    }

    async fn pick_sources(
        &self,
        title: &str,
        candidates: &[WebSearchResult],
    ) -> Result<Vec<WebSearchResult>> {
        #[derive(Deserialize)]
        struct PicksJson {
            #[serde(default)]
            picks: Vec<usize>,
        }

        let mut prompt =
            format!("Title: {title}\n\nCandidate search results (index: title — url — snippet):\n");
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

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: prompt,
        }];
        let options = CompletionOptions {
            max_tokens: Some(150),
            temperature: Some(0.0),
            system_prompt: Some(
                "Reply with ONLY a JSON object of the form {\"picks\": [<indices>]} — no other \
                 text, no markdown fences."
                    .to_string(),
            ),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let json_str = extract_json_object(raw.trim())?;
        let parsed: PicksJson = serde_json::from_str(json_str)
            .map_err(|e| concept_parse_error(&format!("invalid source-pick JSON: {e}"), &raw))?;

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
        #[derive(Deserialize)]
        struct TopicJson {
            topic: String,
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

        let messages = [ChatMessage {
            role: MessageRole::User,
            content: prompt,
        }];
        let options = CompletionOptions {
            max_tokens: Some(1400),
            temperature: Some(0.3),
            system_prompt: Some(RESEARCH_SYNTHESIS_PROMPT.to_string()),
            stop: None,
            cancel: None,
        };

        let raw = self.llm.complete(&messages, &options).await?;
        let trimmed = raw.trim();
        let json_str = extract_json_object(trimmed)?;
        let parsed: SynthesisJson = serde_json::from_str(json_str).map_err(|error| {
            concept_parse_error(
                &format!("invalid research synthesis JSON: {error}"),
                trimmed,
            )
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
}

const RESEARCH_SYNTHESIS_PROMPT: &str = r##"You are a careful research assistant fact-checking and synthesizing information gathered from real, numbered web sources.

The sources may cover a single subject or several related subjects. Identify every distinct topic worth reporting on, and write ONLY claims that are actually supported by the numbered sources provided — do not use outside knowledge, and do not invent facts.

Return a JSON object with:
- "title_answer": if the title poses a question or claim the sources answer or support/refute, one sentence directly answering it, with an inline [n] citation. Otherwise null.
- "topics": an array with one object per distinct topic, each with:
  - "topic": a short label for this specific subject.
  - "summary": a 2-5 sentence paragraph synthesizing what the sources say about this topic. EVERY factual claim must end with an inline citation marker like "[1]" or "[2][4]" pointing at the source number(s) that support it. If sources disagree, say so explicitly (e.g. "one source claims X[1], while another found no such effect[3]").
  - "tags": an array of 1-4 key term objects, each {"term": "...", "qualified": "..." (optional, only for ambiguous bare terms)} — same rules as regular page tagging: prefer a specific verbatim phrase, only add "qualified" when a short generic word would otherwise be ambiguous.

Example: {"title_answer": "Yes, magnesium supplementation shows a modest benefit for sleep onset[1][2].", "topics": [{"topic": "Magnesium and sleep", "summary": "Multiple sources report magnesium glycinate improves sleep onset latency[1], though effect sizes were small in a controlled trial[2]. One source notes benefits may be limited to people who are already magnesium-deficient[3].", "tags": [{"term": "magnesium"}, {"term": "sleep onset"}]}]}

Return ONLY the JSON object, no other text."##;

fn truncate(s: &str, max_chars: usize) -> &str {
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
    struct StubLlm {
        responses: Mutex<std::collections::VecDeque<String>>,
    }

    impl StubLlm {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
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
                let bytes = if url.starts_with("https://search.brave.com/") {
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
        r#"
        <html><body>
        <div class="snippet" data-type="web">
          <a href="https://a.example/article"><div class="title">Source A</div></a>
          <div class="generic-snippet"><div class="content">A snippet about the topic.</div></div>
        </div>
        <div class="snippet" data-type="web">
          <a href="https://b.example/article"><div class="title">Source B</div></a>
          <div class="generic-snippet"><div class="content">Another snippet.</div></div>
        </div>
        </body></html>
        "#
        .to_string()
    }

    fn page_html(title: &str, body: &str) -> String {
        format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
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
}
