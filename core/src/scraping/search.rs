//! Backwards-compatible `web_search`: Brave-then-DuckDuckGo, now driven by the
//! configurable engine registry.
//!
//! This module predates Deep Research and still exists for its original
//! callers (the single-round [`crate::ai::web_research`]), which just want "run
//! a web query, get ranked results" without knowing about the user's engine
//! configuration. Rather than keep a second, hand-rolled Brave/DuckDuckGo
//! scraper in sync with the registry, [`web_search`] now *is* a thin shim over
//! [`crate::scraping::engines`]: it runs the built-in Brave and DuckDuckGo
//! [`SearchEngineDef`](crate::research::config::SearchEngineDef)s through the
//! same generic [`engines::search_one`] every other engine uses, so their
//! selectors and quirks live in exactly one place.
//!
//! Why keep the Brave-then-DuckDuckGo *shape* here at all, when Deep Research
//! has [`engines::search_all`]? Because the two want different failure
//! behaviour. `search_all` merges *all* enabled engines and never retries —
//! right for the agent, which has more rounds to fall back on. `web_search`
//! serves one-shot callers that get a single attempt, so it keeps the original
//! logic that made web search survivable in that context: retry the primary
//! engine through a throttle with a short backoff, then fall back to a second
//! engine, and only surface the primary's error if both come up empty.
//!
//! The result type still lives here ([`SearchResult`]) because it is the shared
//! currency of the whole scraping/research stack; the registry and every engine
//! produce it.

use crate::error::Result;
use crate::research::config::builtin_web_engine;
use crate::scraping::browser::BrowserDriver;
use crate::scraping::engines;

/// One organic web result from a search query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// The short description snippet shown under the result, if any —
    /// useful for an LLM to judge relevance before spending a fetch on it.
    pub snippet: String,
}

/// Runs `query` against Brave's search results page via `browser` and
/// returns up to `limit` organic results in ranked order.
///
/// Results with no resolvable `href` (e.g. ads, "People also ask" widgets
/// without a direct link) are skipped rather than erroring, since a search
/// results page reliably contains some non-result markup alongside the
/// organic listings this function cares about.
pub async fn web_search(
    browser: &dyn BrowserDriver,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // The built-in Brave/DuckDuckGo definitions are the single source of truth
    // for these engines' endpoints and selectors; we run them through the same
    // generic executor as every other engine instead of a bespoke scraper.
    let brave = builtin_web_engine("brave").expect("brave is a built-in engine");
    let duckduckgo = builtin_web_engine("duckduckgo").expect("duckduckgo is a built-in engine");

    // Rate limiting is an expected operating condition here, not an error:
    // a single research run issues several queries back to back, which is
    // exactly the burst pattern engines throttle. Reporting "try again" when
    // the answer was one short pause away is the wrong behaviour, so this
    // retries with a backoff and then tries a second engine.
    let mut last_err = None;
    for attempt in 0..SEARCH_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(retry_delay(attempt)).await;
        }
        match engines::search_one(browser, &brave, query, limit).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            // An empty result set is a real answer ("nothing found"), not a
            // transient failure, so don't burn retries on it — but do let the
            // other engine have a go before accepting it.
            Ok(_) => break,
            Err(err) => last_err = Some(err),
        }
    }

    match engines::search_one(browser, &duckduckgo, query, limit).await {
        Ok(results) if !results.is_empty() => Ok(results),
        // Prefer reporting the primary engine's failure: it's the more
        // informative one, and the fallback failing too usually means the
        // network is down rather than anything specific to DuckDuckGo.
        fallback => match last_err {
            Some(err) => Err(err),
            None => fallback,
        },
    }
}

/// Attempts against the primary engine before falling back.
const SEARCH_ATTEMPTS: u32 = 3;

/// Exponential backoff between attempts. Kept short because a person is
/// waiting on the answer: a research run reads several sources anyway, so a
/// couple of seconds recovering from a throttle is invisible next to that,
/// while a minute of backoff would not be.
fn retry_delay(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_millis(600 * (1 << (attempt - 1)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim: exercises the *real* built-in Brave selectors through the
    /// generic [`engines::parse_html`] the production path now uses, so these
    /// long-standing regression cases keep guarding Brave parsing after the
    /// move to the registry (a broken Brave selector or a parser regression
    /// still fails them). The same-host-drop rule in `parse_html` is what makes
    /// the relative `/local/goggles` link — which resolves back onto
    /// `search.brave.com` — get skipped, reproducing the old "non-http href"
    /// behaviour generically.
    fn parse_brave_results(html: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let selectors = builtin_web_engine("brave")
            .expect("brave is a built-in engine")
            .selectors
            .expect("brave is an HTML engine with selectors");
        engines::parse_html(
            html,
            &selectors,
            "https://search.brave.com/search",
            limit,
            false,
        )
    }

    #[test]
    fn parses_titles_urls_and_snippets_from_brave_results_html() {
        let html = r#"
        <html><body>
        <div class="snippet svelte-jmfu5f" data-pos="0" data-type="web">
          <div class="result-content">
            <a href="https://www.health.com/magnesium-sleep">
              <div class="title search-snippet-title">Magnesium and Sleep: What to Know</div>
            </a>
            <div class="generic-snippet">
              <div class="content">Magnesium may improve sleep quality and insulin sensitivity.</div>
            </div>
          </div>
        </div>
        <div class="snippet svelte-jmfu5f" data-pos="1" data-type="web">
          <div class="result-content">
            <a href="https://example.org/insulin-resistance">
              <div class="title search-snippet-title">Understanding Insulin Resistance</div>
            </a>
            <div class="generic-snippet">
              <div class="content">A guide to insulin resistance and diet.</div>
            </div>
          </div>
        </div>
        </body></html>
        "#;

        let results = parse_brave_results(html, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Magnesium and Sleep: What to Know");
        assert_eq!(results[0].url, "https://www.health.com/magnesium-sleep");
        assert!(results[0].snippet.contains("insulin sensitivity"));
        assert_eq!(results[1].url, "https://example.org/insulin-resistance");
    }

    #[test]
    fn respects_the_limit() {
        let html = r#"
        <html><body>
        <div class="snippet" data-type="web"><a href="https://a.com"><div class="title">A</div></a></div>
        <div class="snippet" data-type="web"><a href="https://b.com"><div class="title">B</div></a></div>
        <div class="snippet" data-type="web"><a href="https://c.com"><div class="title">C</div></a></div>
        </body></html>
        "#;
        let results = parse_brave_results(html, 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn skips_non_http_hrefs() {
        let html = r#"
        <html><body>
        <div class="snippet" data-type="web"><a href="/local/goggles"><div class="title">Internal</div></a></div>
        <div class="snippet" data-type="web"><a href="https://real.com"><div class="title">Real</div></a></div>
        </body></html>
        "#;
        let results = parse_brave_results(html, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://real.com");
    }

    #[test]
    fn no_results_returns_empty_not_error() {
        let results = parse_brave_results("<html><body>No results</body></html>", 10).unwrap();
        assert!(results.is_empty());
    }
}
