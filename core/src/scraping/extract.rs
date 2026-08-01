//! Turns a [`crate::scraping::browser::FetchedResource`] into readable
//! [`PageContent`] — a title, plain-text body, and outgoing links — no
//! matter whether the source was an HTML page or a PDF. Callers ([`crate::scraping::clipper`])
//! only ever deal with `PageContent`, so adding a new source format later
//! (e.g. plain text, EPUB) only means adding a branch here, not touching the
//! crawler.

use scraper::{Html, Selector};
use url::Url;

use crate::error::{CoreError, Result};
use crate::scraping::browser::FetchedResource;

/// A link discovered on a page, with its text kept so an LLM (or a human)
/// can judge relevance without having to fetch the target first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLink {
    pub url: String,
    pub text: String,
}

/// Readable content extracted from one fetched resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContent {
    pub url: String,
    pub title: String,
    /// Plain-text body, stripped of markup/scripts/styles.
    pub text: String,
    /// Outgoing links found on the page (empty for PDFs).
    pub links: Vec<PageLink>,
}

/// Extract [`PageContent`] from `resource`, dispatching on its declared
/// content type (falling back to sniffing the URL's extension, then to
/// treating it as HTML — the common case for a bare `GET`).
pub fn extract(resource: &FetchedResource) -> Result<PageContent> {
    if is_pdf(resource) {
        extract_pdf(resource)
    } else {
        extract_html(resource)
    }
}

fn is_pdf(resource: &FetchedResource) -> bool {
    let content_type_is_pdf = resource
        .content_type
        .as_deref()
        .is_some_and(|ct| ct.contains("application/pdf"));
    let url_looks_like_pdf = resource.url.to_ascii_lowercase().ends_with(".pdf");
    content_type_is_pdf || url_looks_like_pdf
}

fn extract_html(resource: &FetchedResource) -> Result<PageContent> {
    let body = String::from_utf8_lossy(&resource.bytes);
    let document = Html::parse_document(&body);

    let title = select_first_text(&document, "title").unwrap_or_default();

    // Heuristic "reader mode": concatenate the tags that normally carry the
    // actual article text, skipping nav/script/style/header/footer noise
    // that a plain `body` text-node walk would otherwise pull in.
    let content_selector =
        Selector::parse("h1, h2, h3, h4, h5, h6, p, li, blockquote").expect("valid selector");
    let text = document
        .select(&content_selector)
        .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let base = Url::parse(&resource.url).ok();
    let link_selector = Selector::parse("a[href]").expect("valid selector");
    let mut links = Vec::new();
    for el in document.select(&link_selector) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        let resolved = match (&base, Url::parse(href)) {
            (_, Ok(absolute)) => absolute.to_string(),
            (Some(base), Err(_)) => match base.join(href) {
                Ok(joined) => joined.to_string(),
                Err(_) => continue,
            },
            (None, Err(_)) => continue,
        };
        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
        links.push(PageLink {
            url: resolved,
            text,
        });
    }

    Ok(PageContent {
        url: resource.url.clone(),
        title,
        text,
        links,
    })
}

fn extract_pdf(resource: &FetchedResource) -> Result<PageContent> {
    let text = pdf_extract::extract_text_from_mem(&resource.bytes)
        .map_err(|e| CoreError::Other(format!("PDF text extraction failed: {e}")))?;

    let title = resource
        .url
        .rsplit('/')
        .next()
        .unwrap_or(&resource.url)
        .to_string();

    Ok(PageContent {
        url: resource.url.clone(),
        title,
        text,
        links: Vec::new(),
    })
}

fn select_first_text(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html_resource(url: &str, html: &str) -> FetchedResource {
        FetchedResource {
            url: url.to_string(),
            content_type: Some("text/html".to_string()),
            bytes: html.as_bytes().to_vec(),
        }
    }

    #[test]
    fn extracts_title_text_and_resolved_links_from_html() {
        let html = r#"
            <html>
              <head><title>Example Article</title></head>
              <body>
                <nav><a href="/nav-link">Nav</a></nav>
                <script>console.log("noise")</script>
                <h1>Heading</h1>
                <p>First paragraph.</p>
                <p>Second paragraph with a <a href="/related">related link</a>.</p>
              </body>
            </html>
        "#;
        let resource = html_resource("https://example.com/article", html);

        let content = extract(&resource).expect("extraction should succeed");

        assert_eq!(content.title, "Example Article");
        assert!(content.text.contains("Heading"));
        assert!(content.text.contains("First paragraph."));
        assert!(!content.text.contains("console.log"));

        let related = content
            .links
            .iter()
            .find(|l| l.url == "https://example.com/related")
            .expect("relative link should resolve against the page URL");
        assert_eq!(related.text, "related link");
    }

    #[test]
    fn detects_pdf_by_content_type_even_with_a_non_pdf_url() {
        let resource = FetchedResource {
            url: "https://example.com/download?id=1".to_string(),
            content_type: Some("application/pdf".to_string()),
            bytes: Vec::new(),
        };
        assert!(is_pdf(&resource));
    }

    #[test]
    fn detects_pdf_by_url_extension_when_content_type_is_missing() {
        let resource = FetchedResource {
            url: "https://example.com/file.PDF".to_string(),
            content_type: None,
            bytes: Vec::new(),
        };
        assert!(is_pdf(&resource));
    }
}
