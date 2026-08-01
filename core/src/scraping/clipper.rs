//! [`WebClipper`]: fetches a URL (or a small connected set of them), extracts
//! readable content via [`crate::scraping::extract`], and assembles it into
//! one markdown document. Optionally crawls: an [`crate::ai::traits::LlmProvider`]
//! (reused as-is, no bespoke integration) judges whether each page is
//! relevant to the clipper's `goal` and which of its links are worth
//! following next, so a whole small topic-site can be pulled in with one
//! call instead of one URL at a time.

use std::collections::{HashSet, VecDeque};

use serde::Deserialize;

use crate::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};
use crate::scraping::browser::BrowserDriver;
use crate::scraping::extract::{self, PageContent, PageLink};

/// One page's contribution to a clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClippedPage {
    pub url: String,
    pub title: String,
    pub text: String,
}

/// The result of a [`WebClipper::clip`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipResult {
    pub pages: Vec<ClippedPage>,
    /// All pages rendered into a single markdown document, ready to save as
    /// a grafium note.
    pub markdown: String,
}

/// Fetches and (optionally) crawls web/PDF content into markdown.
///
/// Two modes, chosen by which constructor you use:
///   - [`WebClipper::new`]: single page, no LLM required.
///   - [`WebClipper::new`] + [`WebClipper::with_crawl`]: follows links the
///     LLM judges relevant to `goal`, up to `max_pages`/`max_depth`.
pub struct WebClipper<'a> {
    browser: &'a dyn BrowserDriver,
    llm: Option<&'a dyn LlmProvider>,
    goal: String,
    max_pages: usize,
    max_depth: usize,
}

impl<'a> WebClipper<'a> {
    /// A clipper that only ever fetches the one starting URL.
    pub fn new(browser: &'a dyn BrowserDriver, goal: impl Into<String>) -> Self {
        Self {
            browser,
            llm: None,
            goal: goal.into(),
            max_pages: 1,
            max_depth: 0,
        }
    }

    /// Enables multi-page crawling: `llm` decides, per page, whether it's
    /// relevant to `goal` and which of its links to follow next, up to
    /// `max_pages` total pages and `max_depth` link-hops from the start URL.
    pub fn with_crawl(
        mut self,
        llm: &'a dyn LlmProvider,
        max_pages: usize,
        max_depth: usize,
    ) -> Self {
        self.llm = Some(llm);
        self.max_pages = max_pages.max(1);
        self.max_depth = max_depth;
        self
    }

    /// Fetch `start_url` (and, if crawling is enabled, links reachable from
    /// it) and assemble everything judged relevant into one [`ClipResult`].
    pub async fn clip(&self, start_url: &str) -> Result<ClipResult> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back((start_url.to_string(), 0usize));
        let mut pages = Vec::new();

        while let Some((url, depth)) = queue.pop_front() {
            if pages.len() >= self.max_pages || !visited.insert(url.clone()) {
                continue;
            }

            let resource = self.browser.fetch(&url).await?;
            let content = extract::extract(&resource)?;

            let decision = match self.llm {
                Some(llm) => self.ask_llm(llm, &content).await?,
                None => LinkDecision {
                    relevant: true,
                    follow: Vec::new(),
                },
            };

            if decision.relevant {
                pages.push(ClippedPage {
                    url: content.url.clone(),
                    title: content.title.clone(),
                    text: content.text.clone(),
                });
            }

            if depth < self.max_depth {
                for link_url in resolve_follow_targets(&content.links, &decision.follow) {
                    if !visited.contains(&link_url) {
                        queue.push_back((link_url, depth + 1));
                    }
                }
            }
        }

        let markdown = render_markdown(&self.goal, &pages);
        Ok(ClipResult { pages, markdown })
    }

    /// Ask `llm` whether `content` is relevant to `self.goal`, and (if links
    /// were found) which ones to follow next.
    async fn ask_llm(&self, llm: &dyn LlmProvider, content: &PageContent) -> Result<LinkDecision> {
        let prompt = build_prompt(&self.goal, content);
        let messages = [ChatMessage {
            role: MessageRole::User,
            content: prompt,
        }];
        let options = CompletionOptions {
            max_tokens: Some(256),
            temperature: Some(0.0),
            system_prompt: Some(SYSTEM_PROMPT.to_string()),
            stop: None,
        };

        let raw = llm.complete(&messages, &options).await?;
        parse_decision(&raw)
    }
}

const SYSTEM_PROMPT: &str = "You help decide what to keep while clipping web pages into notes. \
Given a goal and one page's extracted text and links, reply with ONLY a JSON object of the form \
{\"relevant\": true|false, \"follow\": [<link indices>]} — no other text, no markdown fences. \
\"relevant\" is whether this page's content should be kept for the goal. \"follow\" lists the \
indices (from the numbered link list) of links worth visiting next to satisfy the goal; use an \
empty list if none are worth following.";

fn build_prompt(goal: &str, content: &PageContent) -> String {
    let mut prompt = format!(
        "Goal: {goal}\n\nPage title: {}\nPage URL: {}\n\nPage text (may be truncated):\n{}\n",
        content.title,
        content.url,
        truncate(&content.text, 4000),
    );

    if !content.links.is_empty() {
        prompt.push_str("\nLinks on this page:\n");
        for (i, link) in content.links.iter().take(40).enumerate() {
            prompt.push_str(&format!(
                "{i}: {} ({})\n",
                truncate(&link.text, 80),
                link.url
            ));
        }
    }

    prompt
}

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LinkDecision {
    relevant: bool,
    #[serde(default)]
    follow: Vec<usize>,
}

/// Parse an LLM's JSON decision, tolerating the common ways models fail to
/// follow "JSON only" instructions (wrapping it in a ```json fence, or
/// prefixing it with a sentence).
fn parse_decision(raw: &str) -> Result<LinkDecision> {
    let trimmed = raw.trim();

    if let Ok(decision) = serde_json::from_str::<LinkDecision>(trimmed) {
        return Ok(decision);
    }

    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    if let (Some(start), Some(end)) = (start, end) {
        if start <= end {
            if let Ok(decision) = serde_json::from_str::<LinkDecision>(&trimmed[start..=end]) {
                return Ok(decision);
            }
        }
    }

    Err(CoreError::Parse(format!(
        "could not parse an LLM relevance decision from: {raw}"
    )))
}

/// Resolve `follow` indices (as chosen by the LLM) back into the actual link
/// URLs, silently ignoring any out-of-range indices instead of failing the
/// whole crawl over a model hallucinating an index.
fn resolve_follow_targets(links: &[PageLink], follow: &[usize]) -> Vec<String> {
    follow
        .iter()
        .filter_map(|&i| links.get(i))
        .map(|link| link.url.clone())
        .collect()
}

fn render_markdown(goal: &str, pages: &[ClippedPage]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Clipped: {goal}\n\n"));
    for page in pages {
        out.push_str(&format!(
            "## {}\n\nSource: {}\n\n{}\n\n",
            page.title, page.url, page.text
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraping::browser::{FetchedResource, MockBrowserDriver};
    use std::collections::HashMap;

    /// A stub `LlmProvider` that returns a canned JSON decision string,
    /// regardless of the prompt — enough to unit-test the crawl loop
    /// without a real model.
    struct StubLlm {
        response: String,
    }

    impl LlmProvider for StubLlm {
        fn complete<'a>(
            &'a self,
            _messages: &'a [ChatMessage],
            _options: &'a CompletionOptions,
        ) -> crate::async_util::BoxFuture<'a, Result<String>> {
            let response = self.response.clone();
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "stub"
        }

        fn health_check<'a>(&'a self) -> crate::async_util::BoxFuture<'a, Result<bool>> {
            Box::pin(async move { Ok(true) })
        }
    }

    fn html_page(url: &str, title: &str, body: &str) -> FetchedResource {
        FetchedResource {
            url: url.to_string(),
            content_type: Some("text/html".to_string()),
            bytes: format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
                .into_bytes(),
        }
    }

    #[tokio::test]
    async fn single_page_clip_without_an_llm_always_keeps_the_page() {
        let mut mock_pages = HashMap::new();
        mock_pages.insert(
            "https://example.com/a".to_string(),
            html_page("https://example.com/a", "A", "<p>Hello from A.</p>"),
        );
        let browser = MockBrowserDriver { pages: mock_pages };

        let clipper = WebClipper::new(&browser, "test goal");
        let result = clipper.clip("https://example.com/a").await.unwrap();

        assert_eq!(result.pages.len(), 1);
        assert_eq!(result.pages[0].title, "A");
        assert!(result.markdown.contains("Hello from A."));
    }

    #[tokio::test]
    async fn crawl_follows_links_the_llm_marks_relevant() {
        let mut mock_pages = HashMap::new();
        mock_pages.insert(
            "https://example.com/a".to_string(),
            html_page(
                "https://example.com/a",
                "A",
                r#"<p>Start page.</p><a href="https://example.com/b">Go to B</a>"#,
            ),
        );
        mock_pages.insert(
            "https://example.com/b".to_string(),
            html_page("https://example.com/b", "B", "<p>Second page content.</p>"),
        );
        let browser = MockBrowserDriver { pages: mock_pages };
        let llm = StubLlm {
            response: r#"{"relevant": true, "follow": [0]}"#.to_string(),
        };

        let clipper = WebClipper::new(&browser, "test goal").with_crawl(&llm, 5, 2);
        let result = clipper.clip("https://example.com/a").await.unwrap();

        assert_eq!(result.pages.len(), 2);
        assert!(result
            .pages
            .iter()
            .any(|p| p.url == "https://example.com/b"));
    }

    #[tokio::test]
    async fn crawl_drops_pages_the_llm_marks_irrelevant() {
        let mut mock_pages = HashMap::new();
        mock_pages.insert(
            "https://example.com/a".to_string(),
            html_page("https://example.com/a", "A", "<p>Off-topic page.</p>"),
        );
        let browser = MockBrowserDriver { pages: mock_pages };
        let llm = StubLlm {
            response: r#"{"relevant": false, "follow": []}"#.to_string(),
        };

        let clipper = WebClipper::new(&browser, "test goal").with_crawl(&llm, 5, 1);
        let result = clipper.clip("https://example.com/a").await.unwrap();

        assert!(result.pages.is_empty());
    }

    #[test]
    fn parses_a_decision_even_when_wrapped_in_a_markdown_code_fence() {
        let raw = "```json\n{\"relevant\": true, \"follow\": [1, 2]}\n```";
        let decision = parse_decision(raw).unwrap();
        assert!(decision.relevant);
        assert_eq!(decision.follow, vec![1, 2]);
    }

    #[test]
    fn out_of_range_follow_indices_are_ignored_not_fatal() {
        let links = vec![PageLink {
            url: "https://example.com/x".to_string(),
            text: "x".to_string(),
        }];
        let resolved = resolve_follow_targets(&links, &[0, 99]);
        assert_eq!(resolved, vec!["https://example.com/x".to_string()]);
    }
}
