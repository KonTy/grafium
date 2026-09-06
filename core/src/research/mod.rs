//! Deep Research: a multi-round, agentic web-research tool built for students.
//!
//! The original "Web Research" ([`crate::ai::web_research`]) is a single pass —
//! plan queries once, search once, read a few pages, synthesize. That is fine
//! for "fact-check this paragraph", but it gives up the moment anything goes
//! sideways: a rate-limited search engine, a page that won't fetch, or a first
//! set of queries that simply missed the good sources all end the run with
//! nothing. A student researching an unfamiliar topic hits exactly those
//! conditions, and "search returned nothing, sorry" is the least useful thing
//! we can hand them.
//!
//! This module keeps the same building blocks — the [`crate::scraping`] browser
//! and extractor, the [`crate::ai::traits::LlmProvider`] abstraction, and the
//! cited-summary shape from [`crate::ai::web_research`] — but wraps them in a
//! loop that behaves like a person doing research: search, read, *ask itself
//! whether it actually has enough to answer*, and if not, work out what's
//! missing and search again with better queries, up to a bounded number of
//! rounds. Every step's prompt is user-editable and the set of search engines
//! is user-configurable, both persisted in [`config::ResearchConfig`], because
//! no fixed prompt or fixed engine list is right for every subject or every
//! student.
//!
//! Layout:
//! - [`config`] — the persisted, user-editable configuration: the search-engine
//!   registry and the per-step prompts, plus the round/source/OCR knobs.
//! - [`agent`] — the [`agent::DeepResearchEngine`] that runs the loop.
//! - [`ocr`] — an opt-in, degrade-gracefully OCR fallback for scanned PDFs.
//!
//! The engine *registry* itself lives in [`crate::scraping::engines`] (next to
//! the browser and HTML/JSON parsing it drives), and is configured from the
//! [`config::SearchEngineDef`]s defined here.

pub mod agent;
pub mod config;
pub mod ocr;

pub use agent::{DeepResearchEngine, ResearchPhase, ResearchProgress};
pub use config::{
    EngineCategory, EngineKind, HtmlSelectors, JsonPaths, ResearchConfig, ResearchPrompts,
    SearchEngineDef,
};
