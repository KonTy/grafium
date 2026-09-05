//! The configurable search-engine *registry*: how a [`SearchEngineDef`] from
//! [`crate::research::config`] is actually turned into results.
//!
//! ## Why this exists
//!
//! The original [`crate::scraping::search`] hard-coded Brave with a DuckDuckGo
//! fallback, each with its own bespoke parser. That is exactly the design that
//! kept breaking: a single engine changing its markup, or rate-limiting this
//! machine's IP, took down "web research" wholesale, and adding an engine meant
//! writing (and maintaining) another hand-rolled scraper. Students don't all
//! want the same sources anyway — a biologist wants Europe PMC, a CS student
//! wants arXiv, a fact-checker wants the open web — so the engine list is user
//! data ([`crate::research::config::ResearchConfig`]), and this module is the
//! single generic executor that runs *any* of them.
//!
//! ## The central lesson: one engine must never fail the run
//!
//! [`search_all`] tries every enabled engine in order and merges their results,
//! **swallowing per-engine failures**. A 429, a DNS error, a page whose markup
//! drifted, a JSON schema that moved — any one of those returns nothing from
//! that engine and the others still contribute. This is the behaviour the old
//! implementation lacked and the reason a research run used to die on a single
//! throttle. [`search_one`] is the strict counterpart used by the Settings
//! "Test" button, where the user *wants* to see the specific failure.
//!
//! ## Two extraction paths, one for each [`EngineKind`]
//!
//! - [`EngineKind::Html`] → [`parse_html`]: a CSS-selector scrape of a rendered
//!   results page. arXiv's Atom **XML** rides this same path — `scraper`/
//!   html5ever parses the feed cleanly, so its element names (`entry`, `id`,
//!   `title`, `summary`) act as "selectors" and we avoid a second XML parser
//!   (see [`crate::research::config::builtin_engines`]). The one wrinkle Atom
//!   needs — a link whose URL is element *text* (`<id>`) rather than an `href`
//!   — is handled by [`parse_html`] falling back to text when there is no href.
//! - [`EngineKind::Json`] → [`parse_json`]: a dotted-path walk over the decoded
//!   response. The real key-less academic APIs (OpenAlex, Crossref, Europe PMC,
//!   Semantic Scholar, DOAJ) are the sources that matter most for students and,
//!   unlike the web engines, are not IP-rate-limited, so getting their quirky
//!   shapes right (array-wrapped fields, OpenAlex's inverted-index abstract) is
//!   where most of the care here goes.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use scraper::{ElementRef, Html, Selector};
use serde_json::{Map, Value};
use url::Url;

use crate::error::{CoreError, Result};
use crate::research::config::{
    EngineKind, HtmlSelectors, JsonPaths, ResearchConfig, SearchEngineDef,
};
use crate::research::EngineCategory;
use crate::scraping::browser::BrowserDriver;
use crate::scraping::search::SearchResult;

/// Cap on the number of words reconstructed from an OpenAlex inverted-index
/// abstract. The snippet only exists to help the model *rank* a candidate
/// before deciding to fetch it, and every candidate's snippet is concatenated
/// into one selection prompt — so an uncapped multi-hundred-word abstract per
/// result would blow that prompt's budget for no benefit. The full text is read
/// later, from the source itself, if the model picks it.
const ABSTRACT_WORD_CAP: usize = 100;

/// Substitute `query` into an engine's `{query}` placeholder, percent-encoding
/// it so the result is a valid URL in **either** a query string or a path
/// segment (DOAJ puts the query in the path: `…/articles/{query}?…`).
///
/// Deliberately dependency-free: `urlencoding` is an optional crate here, gated
/// behind a feature the research module must not force on, and the encoding we
/// need is tiny. We percent-encode everything outside the RFC 3986 *unreserved*
/// set (`A–Z a–z 0–9 - . _ ~`), which is always safe to leave literal anywhere
/// in a URL; notably a space becomes `%20`, never `+` (which only means space
/// in `application/x-www-form-urlencoded`, not in a path).
pub fn build_url(template: &str, query: &str) -> String {
    template.replace("{query}", &percent_encode(query))
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0F));
            }
        }
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Run a **single** engine and surface any failure. Used by the Settings
/// "Test" button, where a bare error ("HTTP 429", "invalid selector") is the
/// point — it tells the student whether their engine definition is wrong or the
/// engine is merely blocking them right now.
pub async fn search_one(
    browser: &dyn BrowserDriver,
    engine: &SearchEngineDef,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let url = build_url(&engine.url_template, query);
    let resource = browser.fetch(&url).await?;
    match engine.kind {
        EngineKind::Json => {
            let paths = engine.json_paths.as_ref().ok_or_else(|| {
                CoreError::Other(format!(
                    "engine '{}' is JSON but has no json_paths configured",
                    engine.id
                ))
            })?;
            parse_json(&resource.bytes, paths, limit)
        }
        EngineKind::Html => {
            let selectors = engine.selectors.as_ref().ok_or_else(|| {
                CoreError::Other(format!(
                    "engine '{}' is HTML but has no selectors configured",
                    engine.id
                ))
            })?;
            let body = String::from_utf8_lossy(&resource.bytes);
            // The *final* URL (after redirects) is what the links on the page
            // are relative to, and whose host we drop as "the engine itself".
            let base = if resource.url.is_empty() {
                url.as_str()
            } else {
                resource.url.as_str()
            };
            parse_html(
                &body,
                selectors,
                base,
                limit,
                engine.category == EngineCategory::Academic,
            )
        }
    }
}

/// Run **every enabled** engine in configured order, merge their results, and
/// dedup by URL — tolerating any individual engine's failure.
///
/// This is the function the whole "never give up on one dead engine" guarantee
/// rests on: an engine that errors (throttled, offline, markup moved) or
/// returns nothing simply contributes nothing, and the run continues. `cancel`
/// is polled between engines so a Stop during the search phase doesn't have to
/// wait out every remaining engine's fetch.
pub async fn search_all(
    browser: &dyn BrowserDriver,
    config: &ResearchConfig,
    query: &str,
    limit: usize,
    cancel: Option<&AtomicBool>,
) -> Vec<SearchResult> {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for engine in config.engines.iter().filter(|e| e.enabled) {
        if crate::ai::web_research::is_cancelled(cancel) {
            break;
        }
        match search_one(browser, engine, query, limit).await {
            Ok(results) => {
                for result in results {
                    if seen.insert(result.url.clone()) {
                        merged.push(result);
                    }
                }
            }
            // The whole point: a single engine failing is expected, not fatal.
            Err(_) => continue,
        }
    }
    merged
}

/// Parse a rendered results page (or an Atom/XML feed) into [`SearchResult`]s
/// using `selectors`, resolving links against `base_url`.
///
/// Shared by every [`EngineKind::Html`] engine and by
/// [`crate::scraping::search`]'s legacy `web_search`, so all the fiddly,
/// repeatedly-rediscovered rules of scraping a results page live in exactly one
/// place:
/// - **href or text.** The link is the element's `href` when present, else its
///   trimmed text — the latter is what lets arXiv's `<id>` (whose text *is* the
///   URL) work without a dedicated XML parser.
/// - **relative → absolute.** Relative and protocol-relative links are resolved
///   against `base_url`.
/// - **redirect unwrapping.** Search engines wrap outbound links in a redirector
///   (DuckDuckGo's `…/l/?uddg=`, Startpage's `?url=`); [`unwrap_redirect`] pulls
///   the real destination back out so citations point at the source, not the
///   engine.
/// - **drop the engine's own host.** Nav/footer/tools links point back at the
///   engine; dropping any result whose host equals `base_url`'s reproduces the
///   old hand-written parsers' "skip internal links" rule generically, for an
///   engine whose chrome we've never seen.
pub fn parse_html(
    html: &str,
    selectors: &HtmlSelectors,
    base_url: &str,
    limit: usize,
    site_native: bool,
) -> Result<Vec<SearchResult>> {
    let document = Html::parse_document(html);
    let result_sel = compile_selector(&selectors.result)?;
    let link_sel = compile_selector(&selectors.link)?;
    let title_sel = compile_selector(&selectors.title)?;
    let snippet_sel = compile_selector(&selectors.snippet)?;

    // A meta-search engine linking to itself is navigation chrome, never a
    // result, so those are dropped. A site-native search is the opposite case:
    // every PubMed hit *is* on pubmed.ncbi.nlm.nih.gov, and applying the same
    // rule there silently discarded all ten results and looked like a broken
    // selector.
    let engine_host = if site_native { None } else { host_of(base_url) };

    let mut results = Vec::new();
    for container in document.select(&result_sel) {
        if results.len() >= limit {
            break;
        }
        let Some(url) = extract_link(&container, &link_sel, base_url) else {
            continue;
        };
        // Never cite the search engine back to itself.
        if let (Some(engine_host), Some(result_host)) = (&engine_host, host_of(&url)) {
            if &result_host == engine_host {
                continue;
            }
        }
        let title = container
            .select(&title_sel)
            .next()
            .map(normalized_text)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| url.clone());
        let snippet = container
            .select(&snippet_sel)
            .next()
            .map(normalized_text)
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }
    Ok(results)
}

/// Parse a JSON API response into [`SearchResult`]s using dotted `paths`.
///
/// The "dotted path" is deliberately the simplest thing that copes with the
/// real academic APIs, each of which nests or array-wraps its fields
/// differently. Two conveniences (see [`resolve_path`]/[`value_to_string`]) do
/// all the work: a path segment that lands on an array transparently steps into
/// its first element (DOAJ's `bibjson.link` is an array of links → first link →
/// `.url`), and a terminal value that is an array of strings takes its first
/// entry (Crossref titles arrive as `["…"]`). OpenAlex ships no plain abstract
/// at all — only an inverted index — so a snippet path landing on that object
/// is reconstructed to text ([`reconstruct_inverted_index`]).
///
/// A missing or non-array `results` path yields an empty list, not an error: an
/// engine that legitimately returned zero hits is a normal outcome the agent
/// must survive, not a failure. Items with no resolvable URL are skipped, since
/// a source that can't be linked can't be cited or fetched.
pub fn parse_json(bytes: &[u8], paths: &JsonPaths, limit: usize) -> Result<Vec<SearchResult>> {
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|e| CoreError::Other(format!("invalid JSON from search engine: {e}")))?;

    let items: &[Value] = match resolve_path(&root, &paths.results) {
        Some(Value::Array(arr)) => arr,
        _ => &[],
    };

    let mut results = Vec::new();
    for item in items {
        if results.len() >= limit {
            break;
        }
        let Some(raw_url) = resolve_string(item, &paths.url) else {
            continue; // no citable/fetchable URL — drop it
        };
        let url = match paths.url_prefix.as_deref() {
            Some(prefix) => join_prefix(prefix, &raw_url),
            None => raw_url,
        };
        // Several APIs return their own search highlighting inline — Crossref
        // wraps titles in `<i>`, Google Patents in `<b>` — which reached the UI
        // as literal markup. Titles and snippets are plain text everywhere they
        // are displayed, so the tags come out here.
        let title = resolve_string(item, &paths.title)
            .map(|t| strip_markup(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| url.clone());
        let snippet = resolve_snippet(item, &paths.snippet)
            .map(|s| strip_markup(&s))
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }
    Ok(results)
}

/// Joins a configured prefix to a relative identifier without doubling or
/// dropping the separating slash.
fn join_prefix(prefix: &str, rest: &str) -> String {
    format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        rest.trim_start_matches('/')
    )
}

/// Removes inline markup and decodes the handful of entities search APIs emit
/// around highlighted terms.
fn strip_markup(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&hellip;", "…")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── HTML helpers ─────────────────────────────────────────────────────────────

fn compile_selector(selector: &str) -> Result<Selector> {
    Selector::parse(selector)
        .map_err(|e| CoreError::Other(format!("invalid CSS selector '{selector}': {e}")))
}

/// Collapse all runs of whitespace to single spaces so a title/snippet pulled
/// from indented XML (arXiv's Atom feed) or nested markup comes out as one clean
/// line rather than carrying the source document's incidental formatting.
fn normalized_text(element: ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_link(
    container: &ElementRef,
    link_selector: &Selector,
    base_url: &str,
) -> Option<String> {
    let element = container.select(link_selector).next()?;
    let raw = match element.value().attr("href") {
        Some(href) if !href.trim().is_empty() => href.trim().to_string(),
        // No href → treat the element's text as the URL (arXiv `<id>`).
        _ => normalized_text(element),
    };
    if raw.is_empty() {
        return None;
    }
    let resolved = resolve_url(&raw, base_url)?;
    let unwrapped = unwrap_redirect(&resolved).unwrap_or(resolved);
    // A "URL" that isn't http(s) after all this (a `javascript:` link, or a
    // title that only looked like text) can't be fetched — drop it rather than
    // hand a garbage citation downstream.
    if unwrapped.starts_with("http://") || unwrapped.starts_with("https://") {
        Some(unwrapped)
    } else {
        None
    }
}

/// Resolve `raw` to an absolute URL string against `base_url`. Absolute http(s)
/// links are returned **verbatim** (no re-serialization), so an engine's exact
/// URL survives untouched; only genuinely relative links are joined.
fn resolve_url(raw: &str, base_url: &str) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    let base = Url::parse(base_url).ok()?;
    base.join(raw).ok().map(|u| u.to_string())
}

/// If `url` is a search-engine redirector wrapping the real destination in a
/// query parameter, return that destination. Best-effort and conservative: it
/// only unwraps when a known wrapper key (`uddg`/`url`/`u`/`q`) holds a value
/// that is itself an http(s) URL, so an ordinary result URL that merely has a
/// `?q=` search parameter is left alone.
fn unwrap_redirect(url: &str) -> Option<String> {
    const WRAPPER_PARAMS: [&str; 4] = ["uddg", "url", "u", "q"];
    let parsed = Url::parse(url).ok()?;
    for (key, value) in parsed.query_pairs() {
        if WRAPPER_PARAMS.contains(&key.as_ref()) {
            let value = value.into_owned();
            if value.starts_with("http://") || value.starts_with("https://") {
                return Some(value);
            }
        }
    }
    None
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

// ── JSON helpers ─────────────────────────────────────────────────────────────

/// Walk a dotted `path` from `root`. Before applying each segment, if the
/// current value is an array we step into its first element — that is what makes
/// `bibjson.link.url` (where `link` is an array) and
/// `fullTextUrlList.fullTextUrl.url` resolve without the caller having to encode
/// array indices in the path.
fn resolve_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        while let Value::Array(items) = current {
            current = items.first()?;
        }
        current = current.get(segment)?;
    }
    Some(current)
}

/// Resolve `path` to a trimmed, non-empty string, coping with the value being a
/// bare string, a number/bool, or an array whose first stringifiable element is
/// what we want (Crossref's `title: ["…"]`).
fn resolve_string(item: &Value, path: &str) -> Option<String> {
    let value = resolve_path(item, path)?;
    value_to_string(value)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        // A terminal array (e.g. Crossref titles) → its first stringifiable item.
        Value::Array(items) => items.iter().find_map(value_to_string),
        Value::Object(_) | Value::Null => None,
    }
}

/// Like [`resolve_string`], but a snippet path landing on an object is treated
/// as an OpenAlex inverted-index abstract and reconstructed to plain text —
/// that API ships the abstract *only* in that form.
fn resolve_snippet(item: &Value, path: &str) -> Option<String> {
    let value = resolve_path(item, path)?;
    if let Value::Object(map) = value {
        let text = reconstruct_inverted_index(map);
        return (!text.is_empty()).then_some(text);
    }
    value_to_string(value)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Reconstruct plain text from an OpenAlex `abstract_inverted_index`: an object
/// mapping each word to the list of positions it occupies. We invert it back to
/// position order and join, capped at [`ABSTRACT_WORD_CAP`] words (see its docs).
fn reconstruct_inverted_index(map: &Map<String, Value>) -> String {
    let mut positioned: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in map {
        if let Value::Array(indices) = positions {
            for index in indices {
                if let Some(pos) = index.as_u64() {
                    positioned.push((pos, word.as_str()));
                }
            }
        }
    }
    positioned.sort_by_key(|(pos, _)| *pos);
    positioned
        .into_iter()
        .take(ABSTRACT_WORD_CAP)
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::config::{builtin_web_engine, EngineCategory};
    use crate::scraping::browser::FetchedResource;
    use crate::scraping::browser::MockBrowserDriver;
    use std::collections::HashMap;

    fn html_selectors() -> HtmlSelectors {
        HtmlSelectors {
            result: "div.result".to_string(),
            link: "a".to_string(),
            title: "a".to_string(),
            snippet: "p".to_string(),
        }
    }

    #[test]
    fn build_url_percent_encodes_the_query_for_path_and_string_positions() {
        assert_eq!(
            build_url("https://x.test/search?q={query}", "rust async traits"),
            "https://x.test/search?q=rust%20async%20traits"
        );
        // Reserved characters that would change a URL's meaning are encoded.
        assert_eq!(
            build_url("https://x.test/api/{query}", "a/b&c=d"),
            "https://x.test/api/a%2Fb%26c%3Dd"
        );
        // Unreserved characters are left literal.
        assert_eq!(
            build_url("https://x.test/?q={query}", "a-b_c.d~e"),
            "https://x.test/?q=a-b_c.d~e"
        );
    }

    #[test]
    fn parse_html_keeps_external_results_and_drops_the_engines_own_host() {
        // Any link that resolves to the engine's own host is chrome (nav,
        // tools, a relative link), not a result — and a *relative* href always
        // resolves back onto the engine host, so it is dropped for the same
        // reason an explicit same-host link is. Only the genuinely external
        // absolute result survives.
        let html = r#"
        <html><body>
          <div class="result"><a href="https://real.test/a">First</a><p>Snippet one.</p></div>
          <div class="result"><a href="/relative/path">Relative</a><p>Snippet two.</p></div>
          <div class="result"><a href="https://engine.test/tools">Internal</a><p>x</p></div>
        </body></html>
        "#;
        let out = parse_html(
            html,
            &html_selectors(),
            "https://engine.test/search",
            10,
            false,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://real.test/a");
        assert_eq!(out[0].title, "First");
        assert_eq!(out[0].snippet, "Snippet one.");
    }

    #[test]
    fn resolve_url_keeps_absolute_verbatim_and_joins_relative_against_base() {
        // Absolute http(s) links are returned byte-for-byte (no trailing-slash
        // normalization that would break exact-match assertions elsewhere).
        assert_eq!(
            resolve_url("https://x.test/a", "https://engine.test/search").as_deref(),
            Some("https://x.test/a")
        );
        // Relative and protocol-relative links resolve against the base.
        assert_eq!(
            resolve_url("/p/q", "https://engine.test/search").as_deref(),
            Some("https://engine.test/p/q")
        );
        assert_eq!(
            resolve_url("//other.test/x", "https://engine.test/search").as_deref(),
            Some("https://other.test/x")
        );
    }

    #[test]
    fn parse_html_unwraps_duckduckgo_style_redirects() {
        let html = r#"
        <html><body>
          <div class="result">
            <a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdest.test%2Fpage&rut=abc">Wrapped</a>
            <p>s</p>
          </div>
        </body></html>
        "#;
        let out = parse_html(
            html,
            &html_selectors(),
            "https://html.duckduckgo.com/html/",
            10,
            false,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://dest.test/page");
    }

    #[test]
    fn parse_html_rides_atom_xml_via_element_name_selectors() {
        // arXiv's Atom feed: the link is the <id> element's *text*, not an href.
        let atom = r#"
        <feed xmlns="http://www.w3.org/2005/Atom">
          <entry>
            <id>http://arxiv.org/abs/2401.00001v1</id>
            <title>A Study of Things</title>
            <summary>We study things at length.</summary>
          </entry>
        </feed>
        "#;
        let selectors = HtmlSelectors {
            result: "entry".to_string(),
            link: "id".to_string(),
            title: "title".to_string(),
            snippet: "summary".to_string(),
        };
        let out = parse_html(
            atom,
            &selectors,
            "http://export.arxiv.org/api/query?search_query=all:things",
            10,
            true,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "http://arxiv.org/abs/2401.00001v1");
        assert_eq!(out[0].title, "A Study of Things");
        assert_eq!(out[0].snippet, "We study things at length.");
    }

    #[test]
    fn parse_html_respects_the_limit() {
        let html = r#"
        <html><body>
          <div class="result"><a href="https://a.test">A</a></div>
          <div class="result"><a href="https://b.test">B</a></div>
          <div class="result"><a href="https://c.test">C</a></div>
        </body></html>
        "#;
        let out = parse_html(html, &html_selectors(), "https://engine.test/", 2, false).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn parse_json_walks_arrays_and_takes_first_string_of_a_title_array() {
        // Shape mirrors Crossref: results at message.items, title is ["…"].
        let body = br#"{
            "message": { "items": [
                { "URL": "https://doi.test/1", "title": ["The Real Title"], "abstract": "A summary." },
                { "URL": "https://doi.test/2", "title": ["Second"] }
            ] }
        }"#;
        let paths = JsonPaths {
            results: "message.items".to_string(),
            url: "URL".to_string(),
            title: "title".to_string(),
            snippet: "abstract".to_string(),
            url_prefix: None,
        };
        let out = parse_json(body, &paths, 10).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "https://doi.test/1");
        assert_eq!(out[0].title, "The Real Title");
        assert_eq!(out[0].snippet, "A summary.");
        // Missing snippet is fine (empty), title still resolved.
        assert_eq!(out[1].title, "Second");
        assert_eq!(out[1].snippet, "");
    }

    #[test]
    fn parse_json_steps_into_nested_arrays_for_a_url() {
        // Shape mirrors DOAJ: bibjson.link is an array; we want the first url.
        let body = br#"{
            "results": [
                { "bibjson": {
                    "title": "Open Article",
                    "abstract": "Abstract text.",
                    "link": [ { "url": "https://journal.test/article" } ]
                } }
            ]
        }"#;
        let paths = JsonPaths {
            results: "results".to_string(),
            url: "bibjson.link.url".to_string(),
            title: "bibjson.title".to_string(),
            snippet: "bibjson.abstract".to_string(),
            url_prefix: None,
        };
        let out = parse_json(body, &paths, 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://journal.test/article");
        assert_eq!(out[0].title, "Open Article");
    }

    #[test]
    fn parse_json_reconstructs_an_openalex_inverted_index_abstract() {
        // OpenAlex ships the abstract only as word -> [positions].
        let body = br#"{
            "results": [
                { "doi": "https://doi.test/x", "title": "Paper",
                  "abstract_inverted_index": { "Magnesium": [0], "improves": [1], "sleep": [2] } }
            ]
        }"#;
        let paths = JsonPaths {
            results: "results".to_string(),
            url: "doi".to_string(),
            title: "title".to_string(),
            snippet: "abstract_inverted_index".to_string(),
            url_prefix: None,
        };
        let out = parse_json(body, &paths, 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].snippet, "Magnesium improves sleep");
    }

    #[test]
    fn parse_json_skips_items_without_a_url_and_survives_a_missing_results_array() {
        let paths = JsonPaths {
            results: "results".to_string(),
            url: "doi".to_string(),
            title: "title".to_string(),
            snippet: "abstract".to_string(),
            url_prefix: None,
        };
        // Item with a null doi is skipped (no citable URL).
        let body = br#"{ "results": [ { "doi": null, "title": "No link" } ] }"#;
        assert!(parse_json(body, &paths, 10).unwrap().is_empty());
        // A response missing the results array entirely is empty, not an error.
        let body = br#"{ "meta": { "count": 0 } }"#;
        assert!(parse_json(body, &paths, 10).unwrap().is_empty());
    }

    fn json_engine(id: &str, url_template: &str) -> SearchEngineDef {
        SearchEngineDef {
            id: id.to_string(),
            name: id.to_string(),
            kind: EngineKind::Json,
            url_template: url_template.to_string(),
            enabled: true,
            builtin: false,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results".to_string(),
                url: "url".to_string(),
                title: "title".to_string(),
                snippet: "snippet".to_string(),
                url_prefix: None,
            }),
        }
    }

    fn json_resource(items: &str) -> FetchedResource {
        FetchedResource {
            url: String::new(),
            content_type: Some("application/json".to_string()),
            bytes: format!("{{\"results\": [{items}]}}").into_bytes(),
        }
    }

    #[tokio::test]
    async fn search_all_tolerates_a_dead_engine_and_dedups_across_the_rest() {
        // Two enabled engines: one whose URL is not served (fetch errors → the
        // "dead" engine), one that returns two results. Plus a duplicate URL
        // across the live engine's own page is deduped.
        let live = json_engine("live", "https://live.test/search?q={query}");
        let dead = json_engine("dead", "https://dead.test/search?q={query}");

        let mut pages = HashMap::new();
        pages.insert(
            build_url(&live.url_template, "q"),
            json_resource(
                r#"{"url":"https://a.test","title":"A","snippet":"sa"},
                   {"url":"https://b.test","title":"B","snippet":"sb"},
                   {"url":"https://a.test","title":"A dup","snippet":"dup"}"#,
            ),
        );
        // Note: dead.test URL intentionally absent → MockBrowserDriver errors.
        let browser = MockBrowserDriver { pages };

        let config = ResearchConfig {
            engines: vec![dead, live],
            ..Default::default()
        };
        let results = search_all(&browser, &config, "q", 10, None).await;

        // The dead engine contributed nothing but didn't kill the run; the live
        // engine's duplicate URL was collapsed.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.test");
        assert_eq!(results[1].url, "https://b.test");
    }

    #[tokio::test]
    async fn search_one_reports_a_json_engines_results() {
        let engine = json_engine("j", "https://j.test/api?q={query}");
        let mut pages = HashMap::new();
        pages.insert(
            build_url(&engine.url_template, "hello world"),
            json_resource(r#"{"url":"https://x.test","title":"X","snippet":"sx"}"#),
        );
        let browser = MockBrowserDriver { pages };
        let out = search_one(&browser, &engine, "hello world", 10)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://x.test");
    }

    #[test]
    fn builtin_web_engine_selectors_compile() {
        // A built-in with a broken selector would fail silently at runtime
        // (search_one erroring, search_all swallowing it), so assert here.
        for id in ["brave", "duckduckgo", "mojeek", "startpage"] {
            let engine = builtin_web_engine(id).expect("built-in exists");
            let selectors = engine.selectors.expect("web engine has selectors");
            compile_selector(&selectors.result).expect("result selector valid");
            compile_selector(&selectors.link).expect("link selector valid");
            compile_selector(&selectors.title).expect("title selector valid");
            compile_selector(&selectors.snippet).expect("snippet selector valid");
        }
    }
}
