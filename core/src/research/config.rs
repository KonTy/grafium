//! Persisted, user-editable configuration for Deep Research: the search-engine
//! registry and the per-agentic-step prompts, plus the round/source/OCR knobs.
//!
//! Two things in a research run are impossible to get right with a hard-coded
//! default for everyone: *which sources to search*, and *how to instruct the
//! model at each step*. A biology student wants PubMed/Europe PMC; a CS student
//! wants arXiv and Semantic Scholar; someone fact-checking a news claim wants
//! plain web engines. And the plan/select/assess/refine/synthesize prompts are
//! exactly the levers that decide whether the run is thorough or lazy, cautious
//! or credulous. So both are data, edited in Settings and persisted here, rather
//! than constants baked into the binary.
//!
//! ## Serialization contract (shared with the UI)
//!
//! The frontend mirrors these types verbatim, so the wire shapes are frozen:
//! every struct serializes with its **snake_case field names** (serde default —
//! no `rename_all`), and the two field-less enums serialize as their
//! **PascalCase variant names** (`"Html"`/`"Json"`, `"Web"`/`"Academic"`).
//! Every scalar field carries `#[serde(default = …)]` so a config written by an
//! older build — one that predates a field — still loads, with the missing
//! field taking its built-in default instead of failing the whole parse and
//! wiping the user's engine list.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// How a [`SearchEngineDef`]'s results are obtained from its endpoint.
///
/// Serializes as `"Html"` / `"Json"` (PascalCase variant names) — the UI
/// depends on those exact strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineKind {
    /// Scrape a rendered results page (or an XML/Atom feed — see arXiv) with the
    /// engine's [`HtmlSelectors`], via the shared `scraper` machinery.
    Html,
    /// Fetch a JSON API response and pull fields out of it with the engine's
    /// [`JsonPaths`] dotted paths.
    Json,
}

/// Coarse grouping for the Settings UI, so the engine table can separate
/// "search the open web" from "search scholarly databases" — which is the
/// distinction a student actually cares about. Serializes as `"Web"` /
/// `"Academic"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EngineCategory {
    #[default]
    Web,
    Academic,
}

/// CSS selectors for scraping an [`EngineKind::Html`] results page.
///
/// The `link` selector is resolved leniently by the parser: it uses the
/// element's `href` when present (a normal `<a>` result link) and otherwise
/// falls back to the element's text — which is what lets arXiv's Atom `<id>`
/// element (whose text *is* the abstract URL) ride the exact same HTML path
/// instead of needing a bespoke XML code path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlSelectors {
    /// Selector for the container element wrapping one whole result.
    pub result: String,
    /// Selector (relative to `result`) for the link element carrying the URL.
    pub link: String,
    /// Selector (relative to `result`) for the title text.
    pub title: String,
    /// Selector (relative to `result`) for the snippet/description text.
    pub snippet: String,
}

/// Dotted JSON paths for pulling results out of an [`EngineKind::Json`]
/// response, resolved by [`crate::scraping::engines`].
///
/// Paths are simple `a.b.c` field walks with two conveniences that make the
/// real academic APIs usable without a bespoke adapter each: a path segment that
/// lands on an array transparently indexes its first element (so DOAJ's
/// `bibjson.link` → first link → `.url` works), and a terminal value that is an
/// array of strings takes its first entry (Crossref titles arrive as
/// `["…"]`). OpenAlex's inverted-index abstract is reconstructed to plain text
/// when a snippet path lands on it, since that API ships no plain abstract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonPaths {
    /// Path to the array of result items, relative to the response root.
    pub results: String,
    /// Path to an item's URL field, relative to one item.
    pub url: String,
    /// Path to an item's title field, relative to one item.
    pub title: String,
    /// Path to an item's snippet/abstract field, relative to one item.
    pub snippet: String,
    /// Prepended to the value at `url` when the API returns a relative
    /// identifier rather than a link.
    ///
    /// Google Patents yields `"patent/US1234567B2/en"` and Open Library yields
    /// `"/works/OL123W"` — citable only once joined to their host. Kept
    /// optional so every existing engine definition keeps deserializing.
    #[serde(default)]
    pub url_prefix: Option<String>,
}

/// A single search source. `kind` decides how results are obtained.
///
/// `builtin` engines ship with Grafium and can be disabled (a student who only
/// wants scholarly sources turns the web engines off) but never deleted, so the
/// defaults are always one "reset" away. User-added engines have `builtin:
/// false` and are fully removable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEngineDef {
    /// Stable slug, e.g. `"openalex"`. Identifies built-ins across upgrades.
    pub id: String,
    /// Human-readable display name for the Settings table.
    pub name: String,
    #[serde(default = "default_engine_kind")]
    pub kind: EngineKind,
    /// Endpoint template with a `{query}` placeholder, percent-encoded on
    /// substitution (so it is safe in either a query string or a path segment).
    pub url_template: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub category: EngineCategory,
    /// Present for [`EngineKind::Html`] engines.
    #[serde(default)]
    pub selectors: Option<HtmlSelectors>,
    /// Present for [`EngineKind::Json`] engines.
    #[serde(default)]
    pub json_paths: Option<JsonPaths>,
}

/// One editable prompt per agentic step. These are used as the *system* prompt
/// for their step; the engine appends the concrete data (the question, the
/// candidate results, the gathered sources) as a separate user message, so a
/// user can rewrite how a step reasons without having to preserve any
/// placeholder — and therefore can't accidentally break the run by editing.
/// Each default still states the exact JSON shape its step must return, because
/// the loop parses that output; "Reset to default" restores it if an edit
/// drifts too far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchPrompts {
    #[serde(default = "default_plan_queries")]
    pub plan_queries: String,
    #[serde(default = "default_select_sources")]
    pub select_sources: String,
    #[serde(default = "default_assess_sufficiency")]
    pub assess_sufficiency: String,
    #[serde(default = "default_refine_queries")]
    pub refine_queries: String,
    #[serde(default = "default_synthesize")]
    pub synthesize: String,
}

impl Default for ResearchPrompts {
    fn default() -> Self {
        Self {
            plan_queries: default_plan_queries(),
            select_sources: default_select_sources(),
            assess_sufficiency: default_assess_sufficiency(),
            refine_queries: default_refine_queries(),
            synthesize: default_synthesize(),
        }
    }
}

/// The whole persisted Deep Research configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchConfig {
    #[serde(default = "builtin_engines")]
    pub engines: Vec<SearchEngineDef>,
    #[serde(default)]
    pub prompts: ResearchPrompts,
    /// Maximum search→read→assess→refine rounds before the loop must
    /// synthesize with whatever it has. Bounds the worst-case run time.
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    /// Ceiling on distinct sources fetched across all rounds — the other bound
    /// on run time, and on how much source text the synthesis prompt carries.
    #[serde(default = "default_max_sources")]
    pub max_sources: usize,
    /// How many results to request per query from each engine.
    #[serde(default = "default_results_per_query")]
    pub results_per_query: usize,
    /// Opt-in OCR of scanned PDFs that yield no extractable text. Defaults off:
    /// it needs an external `tesseract` and is slow, so it is a deliberate
    /// choice, not a surprise cost on every run.
    #[serde(default)]
    pub ocr_enabled: bool,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            engines: builtin_engines(),
            prompts: ResearchPrompts::default(),
            max_rounds: default_max_rounds(),
            max_sources: default_max_sources(),
            results_per_query: default_results_per_query(),
            ocr_enabled: false,
        }
    }
}

impl ResearchConfig {
    /// The on-disk location of the config, next to `ai_config.json`.
    pub fn config_path(data_dir: &Path) -> PathBuf {
        data_dir.join("research_config.json")
    }

    /// Load the config from `<data_dir>/research_config.json`, creating it with
    /// [`Self::default`] on first use.
    ///
    /// A corrupt or unparseable file is a genuine dilemma: silently replacing it
    /// would throw away a user's hand-tuned engines and prompts over a stray
    /// comma, while erroring would leave research unusable until they found and
    /// fixed a JSON file by hand. We take the middle path only for *structurally*
    /// valid JSON — per-field `serde(default)` already heals a file that is
    /// merely missing newer fields — and surface a hard error for JSON that
    /// won't parse at all, so the failure is visible rather than a data-losing
    /// silent reset.
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = Self::config_path(data_dir);
        if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let config: ResearchConfig = serde_json::from_str(&raw)?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save(data_dir)?;
            Ok(config)
        }
    }

    /// Persist the config to `<data_dir>/research_config.json`, creating the
    /// directory if needed.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::config_path(data_dir);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

fn default_engine_kind() -> EngineKind {
    EngineKind::Html
}

fn default_max_rounds() -> usize {
    3
}

fn default_max_sources() -> usize {
    8
}

fn default_results_per_query() -> usize {
    6
}

/// The default web-search selectors, shared by the built-in Brave/Mojeek/etc.
/// definitions and by [`crate::scraping::search`]'s legacy `web_search`.
fn brave_selectors() -> HtmlSelectors {
    HtmlSelectors {
        result: "div.snippet[data-type=\"web\"]".to_string(),
        link: "a[href]".to_string(),
        title: ".title".to_string(),
        snippet: ".snippet-description, .generic-snippet .content".to_string(),
    }
}

fn duckduckgo_selectors() -> HtmlSelectors {
    HtmlSelectors {
        result: "div.result, div.web-result".to_string(),
        link: "a.result__a".to_string(),
        title: "a.result__a".to_string(),
        snippet: "a.result__snippet".to_string(),
    }
}

/// The Brave and DuckDuckGo definitions that back the legacy `web_search`
/// helper, kept here so there is a single source of truth for their selectors.
pub(crate) fn builtin_web_engine(id: &str) -> Option<SearchEngineDef> {
    builtin_engines().into_iter().find(|e| e.id == id)
}

/// All engines shipped enabled by default.
///
/// The web engines are HTML scrapes of no-API-key results pages; the academic
/// engines are free, key-less JSON APIs with generous limits — chosen precisely
/// because a student shouldn't have to register for anything. arXiv is the one
/// exception to the JSON rule: its API returns Atom XML, which we route through
/// the HTML path with XML element names as "selectors" (see [`HtmlSelectors`]),
/// because `scraper`/html5ever parses the feed cleanly and reusing that path is
/// far less code than a second XML parser.
///
/// Deliberately excluded: shadow libraries (LibGen, Anna's Archive, Sci-Hub and
/// mirrors). They distribute copyrighted works without authorization; the schema
/// is user-extensible, so adding one is a user's own decision, not something we
/// ship.
pub fn builtin_engines() -> Vec<SearchEngineDef> {
    vec![
        // ── Web (HTML scrape, no API key) ──────────────────────────────────
        SearchEngineDef {
            id: "brave".to_string(),
            name: "Brave Search".to_string(),
            kind: EngineKind::Html,
            url_template: "https://search.brave.com/search?q={query}".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Web,
            selectors: Some(brave_selectors()),
            json_paths: None,
        },
        SearchEngineDef {
            id: "duckduckgo".to_string(),
            name: "DuckDuckGo".to_string(),
            kind: EngineKind::Html,
            url_template: "https://html.duckduckgo.com/html/?q={query}".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Web,
            selectors: Some(duckduckgo_selectors()),
            json_paths: None,
        },
        SearchEngineDef {
            id: "mojeek".to_string(),
            name: "Mojeek".to_string(),
            kind: EngineKind::Html,
            url_template: "https://www.mojeek.com/search?q={query}".to_string(),
            enabled: false,
            builtin: true,
            category: EngineCategory::Web,
            selectors: Some(HtmlSelectors {
                result: "ul.results-standard li, li.result".to_string(),
                link: "a.title, a.ob".to_string(),
                title: "a.title".to_string(),
                snippet: "p.s, p.result-desc".to_string(),
            }),
            json_paths: None,
        },
        SearchEngineDef {
            id: "startpage".to_string(),
            name: "Startpage".to_string(),
            kind: EngineKind::Html,
            url_template: "https://www.startpage.com/sp/search?query={query}".to_string(),
            enabled: false,
            builtin: true,
            category: EngineCategory::Web,
            selectors: Some(HtmlSelectors {
                result: "div.w-gl__result, .result".to_string(),
                link: "a.w-gl__result-title, a.result-link".to_string(),
                title: "a.w-gl__result-title h3, h3".to_string(),
                snippet: "p.w-gl__description, .description".to_string(),
            }),
            json_paths: None,
        },
        // ── Academic (JSON API, no key) ────────────────────────────────────
        SearchEngineDef {
            id: "openalex".to_string(),
            name: "OpenAlex".to_string(),
            kind: EngineKind::Json,
            url_template: "https://api.openalex.org/works?search={query}&per-page=10".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results".to_string(),
                // OpenAlex's canonical `id` is an api.openalex.org URL that
                // returns JSON, not a readable article; the DOI resolves to the
                // real publisher page a student can actually read and cite.
                url: "doi".to_string(),
                title: "title".to_string(),
                // OpenAlex ships no plain abstract — only an inverted index,
                // which the JSON resolver reconstructs into text.
                snippet: "abstract_inverted_index".to_string(),
                url_prefix: None,
            }),
        },
        SearchEngineDef {
            id: "crossref".to_string(),
            name: "Crossref".to_string(),
            kind: EngineKind::Json,
            url_template: "https://api.crossref.org/works?query={query}&rows=10".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "message.items".to_string(),
                url: "URL".to_string(),
                title: "title".to_string(),
                snippet: "abstract".to_string(),
                url_prefix: None,
            }),
        },
        SearchEngineDef {
            id: "arxiv".to_string(),
            name: "arXiv".to_string(),
            // Atom XML, parsed through the HTML path with element-name
            // "selectors" — see the module and `builtin_engines` docs.
            kind: EngineKind::Html,
            url_template: "http://export.arxiv.org/api/query?search_query=all:{query}&max_results=10"
                .to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: Some(HtmlSelectors {
                result: "entry".to_string(),
                // The <id> element's *text* is the abstract-page URL; the link
                // resolver falls back to element text when there is no href.
                link: "id".to_string(),
                title: "title".to_string(),
                snippet: "summary".to_string(),
            }),
            json_paths: None,
        },
        SearchEngineDef {
            id: "europepmc".to_string(),
            name: "Europe PMC".to_string(),
            kind: EngineKind::Json,
            // `resultType=core` is required for `abstractText` and a resolvable
            // full-text URL to be present at all — lite results carry neither,
            // so without it Europe PMC hits could be neither read nor cited.
            url_template:
                "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={query}&format=json&pageSize=10&resultType=core"
                    .to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "resultList.result".to_string(),
                url: "fullTextUrlList.fullTextUrl.url".to_string(),
                title: "title".to_string(),
                snippet: "abstractText".to_string(),
                url_prefix: None,
            }),
        },
        SearchEngineDef {
            id: "semanticscholar".to_string(),
            name: "Semantic Scholar".to_string(),
            kind: EngineKind::Json,
            url_template:
                "https://api.semanticscholar.org/graph/v1/paper/search?query={query}&fields=title,abstract,url,year&limit=10"
                    .to_string(),
            enabled: false,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "data".to_string(),
                url: "url".to_string(),
                title: "title".to_string(),
                snippet: "abstract".to_string(),
                url_prefix: None,
            }),
        },
        SearchEngineDef {
            id: "doaj".to_string(),
            name: "DOAJ".to_string(),
            kind: EngineKind::Json,
            url_template: "https://doaj.org/api/search/articles/{query}?pageSize=10".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results".to_string(),
                url: "bibjson.link.url".to_string(),
                title: "bibjson.title".to_string(),
                snippet: "bibjson.abstract".to_string(),
                url_prefix: None,
            }),
        },
        // PubMed is scraped rather than driven through NCBI's E-utilities
        // because that API needs two round trips — `esearch` returns bare IDs,
        // then `esummary` turns them into records — and every engine here is
        // one request by construction. The public results page carries stable,
        // semantic class names and is the same data.
        SearchEngineDef {
            id: "pubmed".to_string(),
            name: "PubMed (NIH)".to_string(),
            kind: EngineKind::Html,
            url_template: "https://pubmed.ncbi.nlm.nih.gov/?term={query}".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: Some(HtmlSelectors {
                result: "article.full-docsum".to_string(),
                link: "a.docsum-title".to_string(),
                title: "a.docsum-title".to_string(),
                snippet: ".docsum-snippet, .full-view-snippet".to_string(),
            }),
            json_paths: None,
        },
        // Google Patents rather than a single patent office: it indexes USPTO,
        // EPO, WIPO, CNIPA and others together, and its search endpoint needs
        // no key. The official USPTO APIs now require registration, which would
        // make this useless out of the box.
        SearchEngineDef {
            id: "googlepatents".to_string(),
            name: "Patents (Google Patents)".to_string(),
            kind: EngineKind::Json,
            url_template: "https://patents.google.com/xhr/query?url=q%3D{query}".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results.cluster.result".to_string(),
                url: "id".to_string(),
                title: "patent.title".to_string(),
                snippet: "patent.snippet".to_string(),
                url_prefix: Some("https://patents.google.com".to_string()),
            }),
        },
        SearchEngineDef {
            id: "openlibrary".to_string(),
            name: "Open Library".to_string(),
            kind: EngineKind::Json,
            url_template: "https://openlibrary.org/search.json?q={query}&limit=10".to_string(),
            enabled: true,
            builtin: true,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "docs".to_string(),
                url: "key".to_string(),
                title: "title".to_string(),
                snippet: "first_sentence".to_string(),
                url_prefix: Some("https://openlibrary.org".to_string()),
            }),
        },
    ]
}

// ── Default prompts ─────────────────────────────────────────────────────────
//
// Each is the system prompt for its step and states the JSON shape the loop
// parses. They are written plainly on purpose: a student should be able to open
// Settings, read the prompt, and understand (and tweak) what that step does.

fn default_plan_queries() -> String {
    "You are a research assistant planning how to investigate a question on the web. \
Given the user's question, produce up to 4 focused, diverse search-engine queries that together \
would surface authoritative sources answering it. Prefer specific, well-formed queries over broad \
ones; where the topic is scholarly, include an academic phrasing. Do not answer the question \
yourself. Reply with ONLY a JSON object of the form {\"queries\": [\"...\", \"...\"]} — no other \
text, no markdown fences."
        .to_string()
}

fn default_select_sources() -> String {
    "You are choosing which search results are worth reading in full to answer a research \
question. You are given the question and a numbered list of candidate results (title, URL, \
snippet). Pick the most relevant, credible, and diverse results — avoid near-duplicates and \
low-quality sources. Reply with ONLY a JSON object of the form {\"picks\": [<indices>]} listing \
the indices to read, most useful first — no other text, no markdown fences."
        .to_string()
}

fn default_assess_sufficiency() -> String {
    "You are judging whether the material gathered so far is enough to write a well-supported, \
cited answer to a research question. You are given the question and excerpts from the sources read \
so far. Be honest: if key claims are unsupported, sources conflict without resolution, or an \
important angle is missing, it is NOT sufficient yet. Reply with ONLY a JSON object of the form \
{\"sufficient\": true|false, \"missing\": \"one sentence naming what is still needed (empty if \
sufficient)\"} — no other text, no markdown fences."
        .to_string()
}

fn default_refine_queries() -> String {
    "You are improving a web-research run that does not yet have enough to answer the question. \
You are given the question, the titles already gathered, and a note on what is still missing. \
Produce up to 4 NEW search-engine queries targeting the gap — do not repeat the earlier queries, \
and get more specific or approach from a different angle. Reply with ONLY a JSON object of the form \
{\"queries\": [\"...\", \"...\"]} — no other text, no markdown fences."
        .to_string()
}

fn default_synthesize() -> String {
    "You are a careful research assistant writing the final answer from real, numbered web \
sources. Use ONLY the numbered sources provided — do not add outside knowledge and do not invent \
facts. Identify every distinct topic worth reporting. Return a JSON object with:\n\
- \"title_answer\": if the question can be answered directly, one sentence answering it with an \
inline [n] citation; otherwise null.\n\
- \"topics\": an array of objects, each with \"topic\" (a short label), \"summary\" (a 2-5 \
sentence paragraph where EVERY factual claim ends with an inline citation like \"[1]\" or \
\"[2][4]\"; if sources disagree, say so), and \"tags\" (1-4 key-term objects {\"term\": \"...\"}).\n\
Return ONLY the JSON object, no other text, no markdown fences."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_ships_every_builtin_engine() {
        let config = ResearchConfig::default();
        for id in [
            "brave",
            "duckduckgo",
            "mojeek",
            "startpage",
            "openalex",
            "crossref",
            "arxiv",
            "europepmc",
            "semanticscholar",
            "doaj",
            "pubmed",
            "googlepatents",
            "openlibrary",
        ] {
            let engine = config
                .engines
                .iter()
                .find(|e| e.id == id)
                .unwrap_or_else(|| panic!("missing built-in engine {id}"));
            assert!(engine.builtin, "{id} must be marked built-in");
        }
    }

    /// Engines that actively refuse automated access ship **off**.
    ///
    /// Verified against the live services: Mojeek and Startpage answer a
    /// scripted request with a CAPTCHA/verification page, and Semantic Scholar
    /// returns HTTP 429 to anonymous callers. Leaving them on meant a research
    /// run always carried three engines that could only ever contribute
    /// nothing, and — worse — made "no results" look like a Grafium bug.
    /// Getting past them would mean defeating an access control, which is not
    /// something this ships.
    #[test]
    fn engines_that_block_automation_ship_disabled() {
        let config = ResearchConfig::default();
        for id in ["mojeek", "startpage", "semanticscholar"] {
            let engine = config.engines.iter().find(|e| e.id == id).unwrap();
            assert!(!engine.enabled, "{id} must ship disabled");
            assert!(
                engine.builtin,
                "{id} stays a built-in so it can be re-enabled"
            );
        }
        // Everything else ships ready to use.
        for id in [
            "openalex",
            "crossref",
            "arxiv",
            "europepmc",
            "doaj",
            "pubmed",
            "googlepatents",
        ] {
            let engine = config.engines.iter().find(|e| e.id == id).unwrap();
            assert!(engine.enabled, "{id} ships enabled");
        }
    }

    #[test]
    fn no_shadow_libraries_are_shipped() {
        let config = ResearchConfig::default();
        for engine in &config.engines {
            let haystack = format!("{} {}", engine.id, engine.url_template).to_lowercase();
            for banned in ["libgen", "sci-hub", "scihub", "annas-archive", "anna's"] {
                assert!(
                    !haystack.contains(banned),
                    "shadow library {banned} must never be a built-in"
                );
            }
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = ResearchConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ResearchConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults_so_old_configs_keep_working() {
        // A config written by an older build: only a partial engine list and a
        // single prompt overridden, everything else absent.
        let old = r#"{
            "engines": [
                {"id": "custom", "name": "My Engine", "kind": "Html",
                 "url_template": "https://ex.com/?q={query}", "enabled": true, "builtin": false,
                 "category": "Web",
                 "selectors": {"result": ".r", "link": "a", "title": "h3", "snippet": "p"}}
            ],
            "prompts": {"plan_queries": "custom plan"}
        }"#;

        let config: ResearchConfig = serde_json::from_str(old).unwrap();

        // Present fields are respected.
        assert_eq!(config.engines.len(), 1);
        assert_eq!(config.engines[0].id, "custom");
        assert_eq!(config.prompts.plan_queries, "custom plan");
        // Absent prompt fields fall back to their built-in defaults.
        assert_eq!(config.prompts.synthesize, default_synthesize());
        assert_eq!(
            config.prompts.assess_sufficiency,
            default_assess_sufficiency()
        );
        // Absent scalar knobs fall back, not to `0`/`false` by accident but to
        // the documented defaults.
        assert_eq!(config.max_rounds, 3);
        assert_eq!(config.max_sources, 8);
        assert_eq!(config.results_per_query, 6);
        assert!(!config.ocr_enabled);
    }

    #[test]
    fn missing_engines_field_restores_the_full_builtin_set() {
        // A config with no `engines` key at all must not come back empty —
        // otherwise a stray hand-edit would silently disable all search.
        let config: ResearchConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.engines.len(), builtin_engines().len());
        assert_eq!(config.prompts, ResearchPrompts::default());
    }

    #[test]
    fn load_or_create_writes_defaults_on_first_use_then_reads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = ResearchConfig::config_path(dir.path());
        assert!(!path.exists());

        let created = ResearchConfig::load_or_create(dir.path()).unwrap();
        assert!(path.exists(), "first use writes the file");
        assert_eq!(created, ResearchConfig::default());

        // A subsequent load reads the same content rather than recreating it.
        let loaded = ResearchConfig::load_or_create(dir.path()).unwrap();
        assert_eq!(created, loaded);
    }

    #[test]
    fn enum_variants_serialize_as_pascalcase_for_the_ui() {
        assert_eq!(
            serde_json::to_string(&EngineKind::Html).unwrap(),
            "\"Html\""
        );
        assert_eq!(
            serde_json::to_string(&EngineKind::Json).unwrap(),
            "\"Json\""
        );
        assert_eq!(
            serde_json::to_string(&EngineCategory::Web).unwrap(),
            "\"Web\""
        );
        assert_eq!(
            serde_json::to_string(&EngineCategory::Academic).unwrap(),
            "\"Academic\""
        );
    }
}
