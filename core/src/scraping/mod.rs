//! Web/document "clipper": fetch a URL or PDF, extract its readable content,
//! and (optionally) let an LLM decide which links on the page are worth
//! following next so a whole small topic-site can be pulled into grafium as
//! notes with one command.
//!
//! Kept deliberately generic and dependency-light so it's reusable for
//! anything that needs "fetch a resource, read it, maybe crawl further":
//!   - [`browser::BrowserDriver`] is the only thing that knows how to turn a
//!     URL into bytes. [`browser::HttpBrowserDriver`] does that today with a
//!     plain HTTP client; a future JS-rendering driver (e.g. backed by a
//!     Tauri webview, for sites that need a real browser) can implement the
//!     exact same trait with zero changes anywhere else in this module.
//!   - [`extract`] turns those bytes into readable [`extract::PageContent`]
//!     (title/text/links), regardless of whether the source was HTML or a
//!     PDF — callers don't need to care which.
//!   - [`clipper::WebClipper`] is the only piece that talks to an LLM
//!     ([`crate::ai::traits::LlmProvider`], reused as-is — no new LLM
//!     integration code), and only for the "which links matter" decision
//!     during a multi-page crawl.

pub mod browser;
pub mod clipper;
pub mod extract;

pub use browser::{BrowserDriver, FetchedResource, HttpBrowserDriver};
pub use clipper::{ClipResult, ClippedPage, WebClipper};
pub use extract::PageContent;
