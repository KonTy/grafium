//! [`BrowserDriver`]: the one thing in this module that knows how to turn a
//! URL into raw bytes. Kept as a trait (mirroring [`crate::ai::traits::LlmProvider`]
//! and [`crate::media::transcribe::Transcriber`]) so the rest of the scraping
//! pipeline (`extract`, `clipper`) never depends on *how* a page was fetched —
//! today that's a plain HTTP GET ([`HttpBrowserDriver`]); later it could be a
//! real JS-rendering browser (e.g. a Tauri webview) for sites that need one,
//! swapped in with zero changes anywhere else.

use crate::async_util::BoxFuture;
use crate::error::{CoreError, Result};

/// The raw result of fetching a single URL: enough for [`crate::scraping::extract`]
/// to figure out on its own whether this was HTML, a PDF, or something else.
#[derive(Debug, Clone)]
pub struct FetchedResource {
    /// Final URL after any redirects (used to resolve relative links).
    pub url: String,
    /// The `Content-Type` response header, if any.
    pub content_type: Option<String>,
    /// Raw response body.
    pub bytes: Vec<u8>,
}

/// Anything that can turn a URL into a [`FetchedResource`].
pub trait BrowserDriver: Send + Sync {
    /// Fetch `url` and return its raw bytes plus reported content type.
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>>;
}

/// Default [`BrowserDriver`]: a plain HTTP client. Good enough for static
/// pages, articles, and PDFs; sites that only render content via JavaScript
/// will need a different driver (not implemented yet — this trait exists so
/// that can be added later without touching `extract`/`clipper`).
pub struct HttpBrowserDriver {
    client: reqwest::Client,
}

impl HttpBrowserDriver {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("grafium/clipper")
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for HttpBrowserDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserDriver for HttpBrowserDriver {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
        Box::pin(async move {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("fetch {url} failed: {e}")))?;

            let final_url = response.url().to_string();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);

            if !response.status().is_success() {
                return Err(CoreError::Other(format!(
                    "fetch {url} returned HTTP {}",
                    response.status()
                )));
            }

            let bytes = response
                .bytes()
                .await
                .map_err(|e| CoreError::Other(format!("reading body of {url} failed: {e}")))?
                .to_vec();

            Ok(FetchedResource {
                url: final_url,
                content_type,
                bytes,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub driver used by `clipper`'s tests — returns a fixed page per URL
    /// from an in-memory map instead of hitting the network.
    pub struct MockBrowserDriver {
        pub pages: std::collections::HashMap<String, FetchedResource>,
    }

    impl BrowserDriver for MockBrowserDriver {
        fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
            let result = self
                .pages
                .get(url)
                .cloned()
                .ok_or_else(|| CoreError::NotFound(format!("no mock page for {url}")));
            Box::pin(async move { result })
        }
    }

    #[test]
    fn http_browser_driver_builds_with_a_default_client() {
        // Just confirms construction doesn't panic; no network access in unit tests.
        let _driver = HttpBrowserDriver::new();
    }
}

#[cfg(test)]
pub(crate) use tests::MockBrowserDriver;
