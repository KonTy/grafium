//! Web search via Brave's search results page.
//!
//! Deliberately does NOT use Brave's (or any) paid search API. Brave's
//! `search.brave.com` results page is server-rendered — the actual result
//! titles/URLs/snippets are present in the plain HTML returned by a normal
//! `GET` (verified: the JS on the page only progressively enhances it, it
//! doesn't inject the results client-side) — so it can be fetched with the
//! exact same [`crate::scraping::browser::BrowserDriver`] used everywhere
//! else in this module and parsed with the same `scraper` crate
//! [`crate::scraping::extract`] already depends on. This keeps "Web
//! Research" free of API keys/quotas and consistent with the rest of the
//! scraping module's "no bespoke integration" philosophy.

use scraper::{Html, Selector};

use crate::error::{CoreError, Result};
use crate::scraping::browser::BrowserDriver;

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
    // Brave rate-limits (HTTP 429) after a burst of queries, and one research
    // run issues several in quick succession — so the *first* engine failing
    // is an expected operating condition, not an error to surface. Falling
    // back to a second scrapeable engine keeps research working instead of
    // telling the user to "try again" when the answer was one request away.
    let brave = brave_search(browser, query, limit).await;
    match &brave {
        Ok(results) if !results.is_empty() => return brave,
        _ => {}
    }
    match duckduckgo_search(browser, query, limit).await {
        Ok(results) if !results.is_empty() => Ok(results),
        // Prefer reporting the primary engine's failure: it's the more
        // informative one, and the fallback failing too usually means the
        // network is down rather than anything specific to DuckDuckGo.
        fallback => match brave {
            Err(err) => Err(err),
            Ok(_) => fallback,
        },
    }
}

async fn brave_search(
    browser: &dyn BrowserDriver,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let url = url::Url::parse_with_params("https://search.brave.com/search", &[("q", query)])
        .map_err(|e| CoreError::Other(format!("invalid search query: {e}")))?;
    let resource = browser.fetch(url.as_str()).await?;
    let body = String::from_utf8_lossy(&resource.bytes);
    parse_brave_results(&body, limit)
}

/// DuckDuckGo's no-JavaScript endpoint, whose markup is stable and trivially
/// parseable — the same "scrape a results page, no API key" approach as Brave.
async fn duckduckgo_search(
    browser: &dyn BrowserDriver,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let url = url::Url::parse_with_params("https://html.duckduckgo.com/html/", &[("q", query)])
        .map_err(|e| CoreError::Other(format!("invalid search query: {e}")))?;
    let resource = browser.fetch(url.as_str()).await?;
    let body = String::from_utf8_lossy(&resource.bytes);
    parse_duckduckgo_results(&body, limit)
}

/// Parses DuckDuckGo's HTML endpoint. Result links are wrapped in a redirect
/// (`//duckduckgo.com/l/?uddg=<encoded>`), so the real destination has to be
/// pulled back out of the query string before it's usable as a citation.
fn parse_duckduckgo_results(html: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse("div.result, div.web-result").expect("valid selector");
    let link_selector = Selector::parse("a.result__a").expect("valid selector");
    let snippet_selector = Selector::parse("a.result__snippet").expect("valid selector");

    let mut results = Vec::new();
    for node in document.select(&result_selector) {
        if results.len() >= limit {
            break;
        }
        let Some(link) = node.select(&link_selector).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Some(url) = unwrap_ddg_redirect(href) else {
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let snippet = node
            .select(&snippet_selector)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }
    Ok(results)
}

/// Extracts the real destination from a DuckDuckGo redirect link, or returns
/// a plain `http(s)` href unchanged.
fn unwrap_ddg_redirect(href: &str) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        // Still a redirect when it points back at duckduckgo.com/l/.
        if !href.contains("duckduckgo.com/l/") {
            return Some(href.to_string());
        }
    }
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_string()
    };
    let parsed = url::Url::parse(&absolute).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "uddg")
        .map(|(_, v)| v.into_owned())
}

fn parse_brave_results(html: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let document = Html::parse_document(html);
    let snippet_selector =
        Selector::parse("div.snippet[data-type=\"web\"]").expect("valid selector");
    let link_selector = Selector::parse("a[href]").expect("valid selector");
    let title_selector = Selector::parse(".title").expect("valid selector");
    let description_selector =
        Selector::parse(".snippet-description, .generic-snippet .content").expect("valid selector");

    let mut results = Vec::new();
    for snippet in document.select(&snippet_selector) {
        if results.len() >= limit {
            break;
        }

        let Some(link) = snippet.select(&link_selector).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        if !href.starts_with("http") {
            continue; // skip internal/relative Brave links (e.g. "goggles")
        }

        let title = snippet
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| href.to_string());

        let snippet_text = snippet
            .select(&description_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        results.push(SearchResult {
            title,
            url: href.to_string(),
            snippet: snippet_text,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

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
