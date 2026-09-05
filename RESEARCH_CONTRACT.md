# Deep Research — API contract

Frozen interface so the core and UI can be built in parallel. **Do not change
these names or shapes without saying so explicitly in your report.**

## Rust types (`core/src/research/config.rs`)

```rust
/// A search source. `kind` decides how results are obtained.
pub struct SearchEngineDef {
    pub id: String,            // stable slug, e.g. "openalex"
    pub name: String,          // display name
    pub kind: EngineKind,      // Html | Json
    pub url_template: String,  // "{query}" placeholder, URL-encoded on substitution
    pub enabled: bool,
    pub builtin: bool,         // built-ins can be disabled but not deleted
    pub category: EngineCategory, // Web | Academic
    /// Only for `EngineKind::Html` — CSS selectors for scraping.
    pub selectors: Option<HtmlSelectors>,
    /// Only for `EngineKind::Json` — dotted JSON paths.
    pub json_paths: Option<JsonPaths>,
}

pub enum EngineKind { Html, Json }
pub enum EngineCategory { Web, Academic }

pub struct HtmlSelectors {
    pub result: String,   // container
    pub link: String,
    pub title: String,
    pub snippet: String,
}

/// Dotted paths relative to the response root, e.g. "results" / "title".
pub struct JsonPaths {
    pub results: String,  // array
    pub url: String,      // field within an item
    pub title: String,
    pub snippet: String,
}

/// One editable prompt per agentic step.
pub struct ResearchPrompts {
    pub plan_queries: String,
    pub select_sources: String,
    pub assess_sufficiency: String,
    pub refine_queries: String,
    pub synthesize: String,
}

pub struct ResearchConfig {
    pub engines: Vec<SearchEngineDef>,
    pub prompts: ResearchPrompts,
    pub max_rounds: usize,      // default 3
    pub max_sources: usize,     // default 8
    pub results_per_query: usize, // default 6
    pub ocr_enabled: bool,      // default false
}
```

`ResearchConfig::default()` must supply all built-in engines and default
prompts. Persisted as JSON next to `ai_config.json`, in
`<data_dir>/research_config.json`.

## Built-in engines (defaults)

Web (`EngineKind::Html`):
- `brave` — https://search.brave.com/search?q={query}
- `duckduckgo` — https://html.duckduckgo.com/html/?q={query}
- `mojeek` — https://www.mojeek.com/search?q={query}
- `startpage` — https://www.startpage.com/sp/search?query={query}

Academic (`EngineKind::Json`, no API key, generous free limits):
- `openalex` — https://api.openalex.org/works?search={query}&per-page=10
- `crossref` — https://api.crossref.org/works?query={query}&rows=10
- `arxiv` — http://export.arxiv.org/api/query?search_query=all:{query}&max_results=10  (Atom XML — parse as XML, mark kind Html with XML-ish selectors OR add a dedicated parser; your call, document it)
- `europepmc` — https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={query}&format=json&pageSize=10
- `semanticscholar` — https://api.semanticscholar.org/graph/v1/paper/search?query={query}&fields=title,abstract,url,year&limit=10
- `doaj` — https://doaj.org/api/search/articles/{query}?pageSize=10

**Do NOT add shadow libraries (LibGen, Anna's Archive, Sci-Hub, or mirrors) as
built-ins.** They distribute copyrighted works without authorization. The
schema is user-extensible, so users can add whatever they choose themselves —
that is their decision, not a shipped feature.

## Tauri commands

```
research_get_config() -> ResearchConfigPayload
research_set_config(payload: ResearchConfigPayload) -> ()
research_reset_prompts() -> ResearchConfigPayload   // defaults for prompts only
research_test_engine(engine: SearchEngineDef, query: String)
    -> Result<Vec<SearchResultPayload>, String>     // for the Test button
research_deep(question: String, requestId: String, graphId: Option<String>) -> ()
research_cancel(requestId: String) -> ()
```

`research_deep` streams over existing channels:
- `ai://chat_stream` — `AskStreamChunk { request_id, delta, phase, note, done, error }`
- `ai://chat_sources` — `AskSourcesPayload { request_id, sources, web_sources }`

New phase strings for the UI: `planning`, `searching_web`, `reading_sources`,
`assessing`, `refining`, `synthesizing`.
`note` carries granular progress ("Reading source 2 of 6: <title>").

## Frontend types (`ui/src/lib/research.ts`)

Mirror the Rust payloads exactly, in camelCase where the command uses
`rename_all = "camelCase"` (state clearly which you chose).

```ts
export interface SearchEngineDef { id, name, kind, url_template, enabled, builtin, category, selectors?, json_paths? }
export interface ResearchPrompts { plan_queries, select_sources, assess_sufficiency, refine_queries, synthesize }
export interface ResearchConfig { engines, prompts, max_rounds, max_sources, results_per_query, ocr_enabled }
export function researchGetConfig(): Promise<ResearchConfig>
export function researchSetConfig(config: ResearchConfig): Promise<void>
export function researchResetPrompts(): Promise<ResearchConfig>
export function researchTestEngine(engine: SearchEngineDef, query: string): Promise<SearchResult[]>
export function researchDeep(question, handlers, requestId, graphId?): Promise<void>
```

## UI surfaces

1. **Settings → Research** (new section or component):
   - Engine table: name, category, enabled toggle, Test button, Delete (built-ins
     not deletable), and an "Add engine" form covering every `SearchEngineDef`
     field.
   - Prompt editor: one textarea per `ResearchPrompts` field, each with its own
     "Reset to default", plus numeric inputs for `max_rounds`, `max_sources`,
     `results_per_query`, and an `ocr_enabled` toggle.
   - Explain each step in one plain sentence so a student knows what the prompt
     controls.

2. **ChatView**: a "Research" checkbox directly below the composer. When
   checked, `send()` calls `researchDeep` instead of `aiAskStream`, bypassing
   the intent classifier entirely. Label it so the cost is obvious, e.g.
   "Research (searches the web, slower)".
