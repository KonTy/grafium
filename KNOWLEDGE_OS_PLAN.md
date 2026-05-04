# Grafium Knowledge OS — Architecture & Multi-Phase Plan

## Vision

Transform Grafium from a note-taking app into a **Knowledge Operating System**: a local-first,
privacy-respecting platform where documents are alive — cross-referenced, semantically linked,
and enriched by AI that runs on YOUR hardware or YOUR API keys.

---

## Research Summary: Tana vs What We're Building

### What Tana Does Well
| Feature | How It Works |
|---------|-------------|
| **SuperTags** | Type system for nodes — `#meeting` gets fields like `date`, `attendees`, `decisions` |
| **Fields** | Typed properties on supertags (text, date, reference, select, etc.) |
| **Views** | Same data rendered as table, kanban, calendar, list |
| **Search Nodes** | Live queries that automatically surface matching content |
| **AI Autofill** | Based on context, AI fills in fields (priority, assignee, category) |
| **Knowledge Graph** | Everything is a node, connections are first-class |
| **MCP/API** | External AI tools can read/write the graph |

### Where We Go Further
| Grafium Advantage | Why It Matters |
|-------------------|---------------|
| **Fully local** | No cloud lock-in. Your data, your machine |
| **Multi-graph** | Separate knowledge domains (work, research, PDFs) with cross-references |
| **Dual AI mode** | Local (Ollama/HuggingFace) OR cloud (OpenAI/Anthropic) — user's choice |
| **Markdown source of truth** | Git-versionable, human-readable, portable |
| **External ingest pipeline** | Playwright scraper / OCR → markdown → auto-indexed into a graph |
| **Embedded references** | AI-discovered connections baked into markdown as footnotes |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        Grafium App (Tauri)                       │
├─────────────┬──────────────┬────────────────┬───────────────────┤
│  Editor UI  │  Right Panel │  Graph Views   │  Settings/Config  │
│  (Svelte)   │  (References)│  (Force graph) │  (AI keys, etc.)  │
└──────┬──────┴──────┬───────┴───────┬────────┴────────┬──────────┘
       │             │               │                 │
┌──────▼─────────────▼───────────────▼─────────────────▼──────────┐
│                     grafium-core (Rust library)                   │
├──────────────┬──────────────┬──────────────┬────────────────────┤
│  Parser      │  DB/Index    │  AI Engine   │  Knowledge Graph   │
│  (markdown,  │  (SQLite +   │  (embeddings,│  (entities, rels,  │
│   links,     │   FTS5)      │   LLM calls) │   cross-graph)     │
│   refs)      │              │              │                    │
└──────────────┴──────┬───────┴──────┬───────┴────────────────────┘
                      │              │
              ┌───────▼───────┐  ┌───▼────────────────┐
              │  SQLite DB    │  │  LanceDB (vectors) │
              │  (structure,  │  │  (embeddings,      │
              │   links, FTS) │  │   semantic search) │
              └───────────────┘  └────────────────────┘
                      │
         ┌────────────▼─────────────┐
         │  External Ingest Tool    │
         │  (uses grafium-core lib) │
         │  - Playwright scraper    │
         │  - PDF OCR pipeline      │
         │  - RSS/feed ingestion    │
         └──────────────────────────┘
```

---

## Key Architecture Decisions

### 1. References in Markdown (Footnote Style)

```markdown
The theory of relativity[^ref-1] fundamentally changed physics[^ref-2].

[^ref-1]: Einstein, A. (1905) — See also: [[Spacetime]], [[Physics/Modern]]
[^ref-2]: Related: [[Quantum Mechanics]], [[Newton's Laws]] | Source: graph://physics-papers/relativity.md
```

**Staleness prevention:**
- Each reference gets a `generated_at` timestamp in a companion `.meta.json` (or inline HTML comment)
- On page open, if references are older than configurable threshold (e.g., 7 days), mark as "stale" in UI
- User can trigger "Refresh References" to re-run AI analysis
- On content edit, affected paragraph's references are flagged stale automatically

### 2. Vector Store: LanceDB (Embedded, Modular)

```rust
// Trait-based abstraction — swap implementations later
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert_embeddings(&self, chunks: &[ChunkEmbedding]) -> Result<()>;
    async fn search_similar(&self, query_embedding: &[f32], top_k: usize) -> Result<Vec<SearchResult>>;
    async fn delete_by_source(&self, source_id: &str) -> Result<()>;
}

// Implementations:
// - LanceDbStore (default, embedded)
// - RemoteVectorStore (future: connect to Qdrant/Weaviate via API)
```

**Why LanceDB:**
- Pure Rust, embedded (no separate process)
- Handles millions of vectors efficiently
- Columnar format (fast filtering by metadata)
- Can later expose as a service for remote access

### 3. Dual AI Engine

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, prompt: &str, options: &CompletionOptions) -> Result<String>;
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn name(&self) -> &str;
}

// Implementations:
// - OllamaProvider { base_url: "http://localhost:11434" }
// - OpenAiProvider { api_key, model }
// - AnthropicProvider { api_key, model }
// - HuggingFaceProvider { model_path } (via candle or GGUF)
```

**Configuration (stored in graph settings or app config):**
```toml
[ai]
mode = "local"  # or "cloud"

[ai.local]
provider = "ollama"
model = "llama3.2"
embedding_model = "nomic-embed-text"

[ai.cloud]
provider = "openai"
api_key = "sk-..."
model = "gpt-4o"
embedding_model = "text-embedding-3-small"
```

### 4. Multi-Graph Registry

```rust
pub struct GraphRegistry {
    graphs: Vec<RegisteredGraph>,
    cross_index: CrossGraphIndex,  // unified vector store spanning all graphs
}

pub struct RegisteredGraph {
    id: String,
    name: String,
    path: PathBuf,
    graph_type: GraphType,  // Primary, Reference, Ingested
    last_indexed: DateTime<Utc>,
}
```

**Cross-graph references use a URI scheme:**
```
graph://physics-papers/relativity.md#block-uuid
graph://web-scrapes/arxiv/2024-attention.md
```

### 5. Chunking Strategy for Embeddings

Documents are chunked at the **block level** (which you already have!):
- Each block gets embedded independently
- Metadata stored with each vector: `{ graph_id, page_id, block_id, page_title, heading_context }`
- For cross-referencing: search similar vectors across ALL graphs in the registry

---

## Multi-Phase Implementation Plan

### Phase 1: Right Panel + Reference UI Foundation
**Goal:** Build the reference panel UI and display system — no AI yet, use existing backlinks/forward links.

| Task | Details |
|------|---------|
| Right panel component | Resizable split panel, slides in from right |
| Reference overlay in editor | Detect `[^ref-N]` in rendered markdown, show as superscript numbers |
| Click-to-expand | Clicking a ref number opens right panel to that reference |
| Backlink section | Show existing `[[wikilinks]]` backlinks in the panel |
| Forward link section | Show pages this page links to |
| Panel tabs | "References", "Backlinks", "Related" (empty for now) |
| Mobile: bottom sheet | On Android, references open as a bottom sheet instead |

**Deliverable:** Panel infrastructure, can display existing link data.

---

### Phase 2: AI Engine Backbone + Embeddings Pipeline
**Goal:** Add LanceDB + embedding generation + LLM abstraction layer.

| Task | Details |
|------|---------|
| Add LanceDB dependency | Embed in `grafium-core` |
| `VectorStore` trait | Abstract interface with LanceDB implementation |
| `LlmProvider` trait | Abstract interface |
| Ollama provider | HTTP client to local Ollama (localhost:11434) |
| OpenAI provider | API client with key from settings |
| Anthropic provider | API client with key from settings |
| Embedding pipeline | On-demand: chunk page blocks → embed → store in LanceDB |
| Settings UI for AI | Model selection, API keys, test connection button |
| "Index this page" command | Manual trigger to embed a single page |
| "Index entire graph" command | Background job to embed all pages |

**Deliverable:** Can embed pages and do `search_similar("quantum physics")` → returns relevant blocks from your graph.

---

### Phase 3: AI-Powered Auto-References
**Goal:** When user triggers "Research this page," AI analyzes content and generates references.

| Task | Details |
|------|---------|
| Reference generation prompt | System prompt that extracts entities, concepts, claims from text |
| Cross-graph semantic search | For each entity/concept, find related content across all graphs |
| Reference formatting | Generate markdown footnotes with links to source pages/blocks |
| Staleness tracking | `.meta.json` per page with `references_generated_at` timestamp |
| Incremental updates | On block edit, flag affected references as stale |
| "Refresh references" button | Re-runs AI analysis for stale references |
| Reference quality | Confidence scores, show "high confidence" vs "suggested" |
| Paragraph-level analysis | AI identifies which paragraphs need references |

**Deliverable:** Click "Research" → AI finds connections in your knowledge base → footnotes appear in text.

---

### Phase 4: Structured Knowledge (SuperTags Equivalent)
**Goal:** Add a flexible type/schema system for nodes.

| Task | Details |
|------|---------|
| Schema definitions | YAML/TOML files in `.grafium/schemas/` per graph |
| Tag → Schema binding | `#person` tag triggers field display (name, company, etc.) |
| Field types | Text, Date, Reference (link to another page), Select, Number |
| AI auto-classification | When a page is created, AI suggests tags based on content |
| AI field autofill | Based on page content, AI suggests field values |
| Schema-aware views | Table view of all `#person` nodes showing their fields |
| Template system | Creating a `#meeting` page pre-fills with field structure |

**Deliverable:** Structured, typed pages with AI-assisted classification.

---

### Phase 5: Multi-Graph Registry + Cross-Graph Index
**Goal:** Manage multiple graphs with a unified knowledge layer.

| Task | Details |
|------|---------|
| Graph registry | Config file listing all known graphs |
| Unified vector index | Single LanceDB instance spanning all graphs |
| Cross-graph links | `graph://` URI scheme for references across graphs |
| Graph browser UI | See all registered graphs, their stats, last indexed |
| External ingest API | Extract `grafium-core` as a library crate other tools can use |
| Ingest CLI tool | `grafium-ingest --graph web-research --source ./scraped-pages/` |

**Deliverable:** Multiple graphs as a unified knowledge base.

---

### Phase 6: External Ingest Pipeline (Separate Tool)
**Goal:** Build the scraper/OCR tool that feeds into Grafium graphs.

| Task | Details |
|------|---------|
| PDF → Markdown | OCR pipeline (Tesseract or local model) |
| Web → Markdown | Playwright scraper → readability → markdown |
| RSS/Atom feeds | Subscribe to feeds, auto-ingest new articles |
| Configurable targets | Each source maps to a specific graph |
| Dedup/update logic | Don't re-ingest unchanged content |
| Scheduling | Cron-like background service |

**Deliverable:** Automated knowledge ingestion from external sources.

---

### Phase 7: Advanced Features
**Goal:** Polish and power-user features.

| Task | Details |
|------|---------|
| Graph visualization | Force-directed graph of page connections |
| Search nodes (live queries) | Saved searches that auto-update (like Tana) |
| AI chat with your graph | "Ask a question" → RAG over your entire knowledge base |
| Spaced repetition integration | AI generates flashcards from references |
| Timeline view | See how knowledge evolved over time |
| Conflict resolution | When external tool and Grafium both modify references |

---

## Technical Decisions Still Open

| Question | Options | Leaning Toward |
|----------|---------|----------------|
| Embedding dimensions | 384 (fast) vs 768 (balanced) vs 1536 (OpenAI) | 768 (nomic-embed-text) for local, configurable for cloud |
| Chunk overlap | 0 (block-level is natural) vs sliding window | Block-level (already have blocks!) |
| Reference update trigger | File watcher vs on-open vs manual | On-open + manual (lazy, resource-friendly) |
| Schema storage format | YAML in `.grafium/` vs SQLite table vs frontmatter | YAML files (human-editable, git-friendly) |
| Cross-graph vector isolation | One big LanceDB vs per-graph tables | One LanceDB, metadata-filtered per graph |
| Mobile AI | Skip entirely vs on-demand only (no background) | On-demand only, cloud providers only on mobile |

---

## Dependency Additions (Estimated)

```toml
# Phase 2
lancedb = "0.x"           # Vector store
arrow = "53"              # LanceDB's data format
reqwest = "0.12"          # HTTP client for Ollama/OpenAI/Anthropic
tokenizers = "0.x"        # Text chunking (HuggingFace tokenizer)
serde_json = "1"          # Already present

# Phase 4
serde_yaml = "0.9"        # Schema definitions

# Phase 6 (separate crate)
tesseract = "0.x"         # OCR
readability = "0.x"       # Web content extraction
```

---

## File Structure (Projected)

```
grafium/
├── core/
│   └── src/
│       ├── ai/
│       │   ├── mod.rs           # AI engine orchestration
│       │   ├── providers/
│       │   │   ├── mod.rs
│       │   │   ├── ollama.rs
│       │   │   ├── openai.rs
│       │   │   └── anthropic.rs
│       │   ├── embeddings.rs    # Chunking + embedding pipeline
│       │   ├── references.rs    # Reference generation logic
│       │   └── traits.rs        # LlmProvider, VectorStore traits
│       ├── knowledge/
│       │   ├── mod.rs
│       │   ├── vector_store.rs  # LanceDB implementation
│       │   ├── cross_graph.rs   # Multi-graph registry + search
│       │   ├── schemas.rs       # SuperTag schema system
│       │   └── entities.rs      # Entity extraction + linking
│       ├── db/                  # (existing)
│       ├── parser/              # (existing, extended for [^ref] parsing)
│       └── graph.rs             # (existing, extended)
├── ingest/                      # Separate crate for external ingestion
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── ocr.rs
│       ├── scraper.rs
│       └── feeds.rs
└── ui/
    └── src/
        └── components/
            ├── ReferencePanel.svelte    # Right panel
            ├── ReferenceOverlay.svelte  # Inline ref markers
            ├── AISettings.svelte        # AI configuration
            ├── GraphBrowser.svelte      # Multi-graph management
            └── SchemaEditor.svelte      # Tag schema editor
```

---

## Next Steps

1. **Review this plan** — ask me any questions
2. **Confirm Phase 1 scope** — I'll start building the right panel
3. **Decide**: Do we want a simple CSS split-panel, or a full drag-to-resize panel system?
